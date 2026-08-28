//! Optional static type checking for annotated UCL programs.
//!
//! The v2 checker deliberately preserves UCL's dynamic behavior for entirely
//! unannotated programs. An annotation activates checking for its initializer
//! or function body; `--strict-types` activates checking for the whole source
//! and requires complete function signatures.

use std::collections::HashMap;
use std::fmt;

use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::parser::{AstKind, AstNode, BinaryOperator, TypeAnnotation, TypeName};
use crate::source::{SourceFile, Span};

use super::BuiltinFunction;

/// The maximum number of AST nodes one static check may visit.
///
/// This independent compile-time budget bounds pathological annotated inputs
/// without changing the evaluator's existing runtime resource limits.
const MAX_TYPECHECK_NODES: usize = 1_000_000;

/// A static type understood by UCL v2's optional checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// A value whose type is not statically known.
    Unknown,
    /// A signed 64-bit integer, written `int` in source.
    Integer,
    /// A boolean, written `bool` in source.
    Boolean,
    /// A UTF-8 string, written `string` in source.
    String,
    /// An immutable list, written `list` in source.
    List,
    /// A callable function, written `function` in source.
    Function,
    /// The absence of a meaningful value, written `unit` in source.
    Unit,
    /// An imported module namespace, written `module` in source.
    Module,
}

impl Type {
    /// Converts a parsed source annotation to the corresponding static type.
    pub(crate) fn from_name(name: TypeName) -> Self {
        match name {
            TypeName::Integer => Self::Integer,
            TypeName::Boolean => Self::Boolean,
            TypeName::String => Self::String,
            TypeName::List => Self::List,
            TypeName::Function => Self::Function,
            TypeName::Unit => Self::Unit,
            TypeName::Module => Self::Module,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Unknown => "unknown",
            Self::Integer => "int",
            Self::Boolean => "bool",
            Self::String => "string",
            Self::List => "list",
            Self::Function => "function",
            Self::Unit => "unit",
            Self::Module => "module",
        };
        f.write_str(text)
    }
}

/// A declared callable signature retained for checking statically known calls.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FunctionSignature {
    parameters: Vec<Type>,
    return_type: Type,
}

/// One static binding in a [`TypeContext`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding {
    ty: Type,
    signature: Option<FunctionSignature>,
}

impl Binding {
    fn new(ty: Type) -> Self {
        Self {
            ty,
            signature: None,
        }
    }

    fn function(signature: FunctionSignature) -> Self {
        Self {
            ty: Type::Function,
            signature: Some(signature),
        }
    }
}

/// The lexical type bindings retained across one evaluation session.
///
/// A context can be shared by repeated calls to
/// [`super::Evaluator::evaluate_typed_in`], which allows annotated declarations
/// in a REPL to inform later typed expressions. The context is intentionally
/// independent from runtime values: it holds only compile-time metadata.
#[derive(Clone, Debug)]
pub struct TypeContext {
    scopes: Vec<HashMap<String, Binding>>,
}

impl TypeContext {
    /// Creates an empty type context with one persistent global scope.
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: String, binding: Binding) {
        self.scopes
            .last_mut()
            .expect("the type context always has one scope")
            .insert(name, binding);
    }

    fn lookup(&self, name: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    /// Returns whether this context has persisted static bindings.
    pub(crate) fn has_bindings(&self) -> bool {
        self.scopes.iter().any(|scope| !scope.is_empty())
    }
}

impl Default for TypeContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs optional static checking over an already parsed source tree.
///
/// Returns `true` if no type errors were added to `sink`. Existing diagnostics
/// do not affect the return value, so callers can safely use one sink for the
/// lexing, parsing, checking, and evaluation pipeline.
pub(crate) fn check(
    root: &AstNode,
    source: &SourceFile,
    context: &mut TypeContext,
    sink: &mut DiagnosticSink,
    strict: bool,
) -> bool {
    // v1 programs deliberately bypass the checker altogether. Apart from
    // preserving purely dynamic semantics, this avoids recursively walking a
    // legal, arbitrarily long left-associated binary chain that the evaluator
    // already handles iteratively.
    let has_annotations = requires_check(root);
    // A retained context keeps annotations meaningful across REPL or embedded
    // evaluations: later unannotated assignments cannot silently invalidate a
    // binding whose type was already declared.
    let has_prior_types = context.has_bindings();
    if !strict && !has_annotations && !has_prior_types {
        return true;
    }
    let baseline = sink.len();
    // Checking must be transactional for persistent environments (notably the
    // REPL): a rejected declaration must not affect later source snippets.
    let saved_context = context.clone();
    let mut checker = TypeChecker {
        source,
        context,
        sink,
        strict,
        expected_returns: Vec::new(),
        remaining_nodes: MAX_TYPECHECK_NODES,
        exhausted: false,
    };
    let _ = checker.node(root, strict || has_annotations || has_prior_types);
    let success = !checker
        .sink
        .iter()
        .skip(baseline)
        .any(|diagnostic| diagnostic.severity == crate::diagnostic::Severity::Error);
    if !success {
        *checker.context = saved_context;
    }
    success
}

/// The stateful implementation of the annotated-source checker.
struct TypeChecker<'a> {
    source: &'a SourceFile,
    context: &'a mut TypeContext,
    sink: &'a mut DiagnosticSink,
    strict: bool,
    expected_returns: Vec<Type>,
    remaining_nodes: usize,
    exhausted: bool,
}

impl TypeChecker<'_> {
    fn node(&mut self, node: &AstNode, active: bool) -> Type {
        if self.exhausted {
            return Type::Unknown;
        }
        if self.remaining_nodes == 0 {
            self.exhausted = true;
            self.error(
                node.span,
                "type error: type checking exceeded its compile-time work budget",
            );
            return Type::Unknown;
        }
        self.remaining_nodes -= 1;
        match &node.kind {
            AstKind::Program { statements } => self.sequence(statements, active, false),
            AstKind::Block { statements } => {
                self.context.push_scope();
                let result = self.sequence(statements, active, true);
                self.context.pop_scope();
                result
            }
            AstKind::Let {
                name,
                annotation,
                value,
                ..
            } => {
                let expected = annotation
                    .as_deref()
                    .map(|annotation| Type::from_name(annotation.name));
                let value_type = self.node(value, active || expected.is_some());
                if let Some(expected) = expected {
                    self.expect(expected, value_type, value.span, "initializer");
                }
                let binding_type = if active {
                    expected.unwrap_or(value_type)
                } else {
                    Type::Unknown
                };
                let name = self.name(*name, "declaration");
                self.context.define(name, Binding::new(binding_type));
                Type::Unit
            }
            AstKind::Assignment { target, value } => {
                let (name, expected) = match &target.kind {
                    AstKind::Identifier => {
                        let name = self.name(target.span, "assignment target");
                        let expected = self.context.lookup(&name).map(|binding| binding.ty);
                        (Some(name), expected)
                    }
                    _ => (None, None),
                };
                let value_type = self.node(
                    value,
                    active || expected.is_some_and(|ty| ty != Type::Unknown),
                );
                if let Some(expected) = expected {
                    self.expect(expected, value_type, value.span, "assignment");
                }
                if name.is_none() && active {
                    self.error(
                        target.span,
                        "type error: assignment target must be an identifier",
                    );
                }
                Type::Unit
            }
            AstKind::Identifier => self
                .context
                .lookup(&self.name(node.span, "identifier"))
                .map_or(Type::Unknown, |binding| binding.ty),
            AstKind::Integer => Type::Integer,
            AstKind::BooleanLiteral(_) => Type::Boolean,
            AstKind::StringLiteral => Type::String,
            AstKind::List { elements } => {
                for element in elements {
                    let _ = self.node(element, active);
                }
                Type::List
            }
            AstKind::Group { expression } => self.node(expression, active),
            AstKind::Unary { operator, operand } => {
                let operand_type = self.node(operand, active);
                if !active {
                    return Type::Unknown;
                }
                match operator {
                    '+' | '-' => {
                        self.expect(
                            Type::Integer,
                            operand_type,
                            operand.span,
                            "unary arithmetic",
                        );
                        Type::Integer
                    }
                    '!' => {
                        self.expect(
                            Type::Boolean,
                            operand_type,
                            operand.span,
                            "logical negation",
                        );
                        Type::Boolean
                    }
                    _ => Type::Unknown,
                }
            }
            AstKind::Binary { .. } => self.binary_chain(node, active),
            AstKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition_type = self.node(condition, active);
                if active {
                    self.expect(
                        Type::Boolean,
                        condition_type,
                        condition.span,
                        "if condition",
                    );
                }
                let then_type = self.node(then_branch, active);
                let else_type = else_branch
                    .as_ref()
                    .map_or(Type::Unit, |branch| self.node(branch, active));
                if active
                    && then_type != Type::Unknown
                    && else_type != Type::Unknown
                    && then_type != else_type
                {
                    self.error(
                        node.span,
                        format!(
                            "type error: `if` branches produce incompatible types `{then_type}` and `{else_type}`"
                        ),
                    );
                    Type::Unknown
                } else if then_type == else_type {
                    then_type
                } else {
                    Type::Unknown
                }
            }
            AstKind::While { condition, body } => {
                let condition_type = self.node(condition, active);
                if active {
                    self.expect(
                        Type::Boolean,
                        condition_type,
                        condition.span,
                        "while condition",
                    );
                }
                let _ = self.node(body, active);
                Type::Unit
            }
            AstKind::For {
                variable,
                start,
                end,
                body,
            } => {
                let item_type = if let Some(start) = start {
                    let start_type = self.node(start, active);
                    let end_type = self.node(end, active);
                    if active {
                        self.expect(Type::Integer, start_type, start.span, "range start");
                        self.expect(Type::Integer, end_type, end.span, "range end");
                    }
                    Type::Integer
                } else {
                    let iterable_type = self.node(end, active);
                    match iterable_type {
                        Type::String => Type::String,
                        Type::List | Type::Unknown => Type::Unknown,
                        other if active => {
                            self.error(
                                end.span,
                                format!(
                                    "type error: `for` expects `string` or `list`, found `{other}`"
                                ),
                            );
                            Type::Unknown
                        }
                        _ => Type::Unknown,
                    }
                };
                self.context.push_scope();
                self.context.define(
                    self.name(*variable, "loop variable"),
                    Binding::new(item_type),
                );
                let _ = self.node(body, active);
                self.context.pop_scope();
                Type::Unit
            }
            AstKind::Function {
                name,
                parameters,
                return_type,
                body,
                ..
            } => self.function(node, *name, parameters, return_type, body, active),
            AstKind::Call { callee, arguments } => self.call(callee, arguments, active),
            AstKind::Index { object, index } => {
                let object_type = self.node(object, active);
                let index_type = self.node(index, active);
                if !active {
                    return Type::Unknown;
                }
                self.expect(Type::Integer, index_type, index.span, "index");
                match object_type {
                    Type::String => Type::String,
                    Type::List | Type::Unknown => Type::Unknown,
                    other => {
                        self.error(object.span, format!("type error: cannot index `{other}`"));
                        Type::Unknown
                    }
                }
            }
            AstKind::Member { object, .. } => {
                let object_type = self.node(object, active);
                if active {
                    self.expect(Type::Module, object_type, object.span, "member access");
                }
                Type::Unknown
            }
            AstKind::Return { value } => {
                let expected = self
                    .expected_returns
                    .last()
                    .copied()
                    .unwrap_or(Type::Unknown);
                let value_type = value.as_ref().map_or(Type::Unit, |value| {
                    self.node(value, active || expected != Type::Unknown)
                });
                if active || expected != Type::Unknown {
                    self.expect(expected, value_type, node.span, "return value");
                }
                Type::Unit
            }
            AstKind::Break | AstKind::Continue | AstKind::Use { .. } => Type::Unit,
        }
    }

    /// Checks a left-associated binary chain iteratively, matching evaluator
    /// behavior and avoiding a stack overflow for a legal long expression.
    fn binary_chain(&mut self, node: &AstNode, active: bool) -> Type {
        let mut links = Vec::new();
        let mut current = node;
        while let AstKind::Binary {
            operator,
            left,
            right,
        } = &current.kind
        {
            links.push((*operator, right.as_ref(), current.span));
            current = left;
        }
        let mut result = self.node(current, active);
        for (operator, right, span) in links.into_iter().rev() {
            let right_type = self.node(right, active);
            result = if active {
                self.binary(operator, result, right_type, span)
            } else {
                Type::Unknown
            };
        }
        result
    }

    fn sequence(&mut self, statements: &[AstNode], active: bool, _scoped: bool) -> Type {
        let mut result = Type::Unit;
        for statement in statements {
            result = self.node(statement, active);
        }
        result
    }

    fn function(
        &mut self,
        node: &AstNode,
        name: Option<Span>,
        parameters: &[crate::parser::Parameter],
        return_type: &Option<Box<TypeAnnotation>>,
        body: &AstNode,
        active: bool,
    ) -> Type {
        let parameter_types = parameters
            .iter()
            .map(|parameter| {
                parameter
                    .annotation
                    .as_deref()
                    .map(|annotation| Type::from_name(annotation.name))
                    .unwrap_or(Type::Unknown)
            })
            .collect::<Vec<_>>();
        let declared_return = return_type
            .as_deref()
            .map(|annotation| Type::from_name(annotation.name))
            .unwrap_or(Type::Unknown);
        let has_annotation = return_type.is_some()
            || parameters
                .iter()
                .any(|parameter| parameter.annotation.is_some());
        if self.strict && !has_annotation {
            self.error(
                node.span,
                "type error: `--strict-types` requires an annotated function signature",
            );
        }
        let signature = FunctionSignature {
            parameters: parameter_types.clone(),
            return_type: declared_return,
        };
        if let Some(name) = name {
            self.context.define(
                self.name(name, "function name"),
                Binding::function(signature),
            );
        }
        let body_active = active || has_annotation || self.strict;
        self.context.push_scope();
        for (parameter, ty) in parameters.iter().zip(parameter_types) {
            self.context
                .define(self.name(parameter.name, "parameter"), Binding::new(ty));
        }
        self.expected_returns.push(declared_return);
        let body_type = self.node(body, body_active);
        self.expected_returns.pop();
        self.context.pop_scope();
        // Explicit `return` statements are checked individually. Only an
        // implicit final expression needs to satisfy the declared result.
        if declared_return != Type::Unknown && !ends_with_return(body) {
            self.expect(declared_return, body_type, body.span, "function result");
        }
        Type::Function
    }

    fn call(&mut self, callee: &AstNode, arguments: &[AstNode], active: bool) -> Type {
        if let AstKind::Identifier = callee.kind {
            let name = self.name(callee.span, "function");
            if let Some(builtin) = builtin(&name) {
                return self.builtin_call(builtin, arguments, active);
            }
            if let Some(binding) = self.context.lookup(&name).cloned()
                && let Some(signature) = binding.signature
            {
                for (index, argument) in arguments.iter().enumerate() {
                    let expected = signature
                        .parameters
                        .get(index)
                        .copied()
                        .unwrap_or(Type::Unknown);
                    let actual = self.node(argument, active || expected != Type::Unknown);
                    if active || expected != Type::Unknown {
                        self.expect(expected, actual, argument.span, "function argument");
                    }
                }
                if active && arguments.len() != signature.parameters.len() {
                    self.error(
                        callee.span,
                        format!(
                            "type error: `{name}` expects {} argument(s), received {}",
                            signature.parameters.len(),
                            arguments.len()
                        ),
                    );
                }
                return signature.return_type;
            }
        }
        let callee_type = self.node(callee, active);
        for argument in arguments {
            let _ = self.node(argument, active);
        }
        if active {
            self.expect(Type::Function, callee_type, callee.span, "call target");
        }
        Type::Unknown
    }

    fn builtin_call(
        &mut self,
        builtin: BuiltinFunction,
        arguments: &[AstNode],
        active: bool,
    ) -> Type {
        let argument_types = arguments
            .iter()
            .map(|argument| self.node(argument, active))
            .collect::<Vec<_>>();
        if !active {
            return Type::Unknown;
        }
        let name = builtin.name();
        let exact = |checker: &mut Self, count: usize| {
            if argument_types.len() != count {
                checker.error(
                    Span::new(0, 0),
                    format!(
                        "type error: `{name}` expects {count} argument(s), received {}",
                        argument_types.len()
                    ),
                );
            }
        };
        match builtin {
            BuiltinFunction::Len => {
                exact(self, 1);
                self.expect_one_of(&argument_types, 0, &[Type::String, Type::List], name);
                Type::Integer
            }
            BuiltinFunction::Str | BuiltinFunction::Type => {
                exact(self, 1);
                Type::String
            }
            BuiltinFunction::Upper | BuiltinFunction::Lower | BuiltinFunction::Trim => {
                exact(self, 1);
                self.expect_argument(Type::String, &argument_types, 0, name);
                Type::String
            }
            BuiltinFunction::Contains => {
                exact(self, 2);
                if let Some(first) = argument_types.first().copied() {
                    match first {
                        Type::String => {
                            self.expect_argument(Type::String, &argument_types, 1, name)
                        }
                        Type::List | Type::Unknown => {}
                        other => self.error(
                            Span::new(0, 0),
                            format!(
                                "type error: `{name}` expects `string` or `list`, found `{other}`"
                            ),
                        ),
                    }
                }
                Type::Boolean
            }
            BuiltinFunction::Int => {
                exact(self, 1);
                self.expect_one_of(&argument_types, 0, &[Type::Integer, Type::String], name);
                Type::Integer
            }
            BuiltinFunction::Find => {
                exact(self, 2);
                if let Some(first) = argument_types.first().copied() {
                    match first {
                        Type::String => {
                            self.expect_argument(Type::String, &argument_types, 1, name)
                        }
                        Type::List | Type::Unknown => {}
                        other => self.error(
                            Span::new(0, 0),
                            format!(
                                "type error: `{name}` expects `string` or `list`, found `{other}`"
                            ),
                        ),
                    }
                }
                Type::Integer
            }
            BuiltinFunction::Replace => {
                exact(self, 3);
                for index in 0..3 {
                    self.expect_argument(Type::String, &argument_types, index, name);
                }
                Type::String
            }
            BuiltinFunction::Slice => {
                exact(self, 3);
                self.expect_one_of(&argument_types, 0, &[Type::String, Type::List], name);
                self.expect_argument(Type::Integer, &argument_types, 1, name);
                self.expect_argument(Type::Integer, &argument_types, 2, name);
                match argument_types.first().copied().unwrap_or(Type::Unknown) {
                    Type::String => Type::String,
                    Type::List => Type::List,
                    _ => Type::Unknown,
                }
            }
            BuiltinFunction::Append => {
                exact(self, 2);
                self.expect_argument(Type::List, &argument_types, 0, name);
                Type::List
            }
        }
    }

    fn binary(&mut self, operator: BinaryOperator, left: Type, right: Type, span: Span) -> Type {
        use BinaryOperator as Op;
        match operator {
            Op::Add => {
                if left == Type::Unknown || right == Type::Unknown {
                    return Type::Unknown;
                }
                if left == right && matches!(left, Type::Integer | Type::String | Type::List) {
                    left
                } else {
                    self.binary_error(operator, left, right, span);
                    Type::Unknown
                }
            }
            Op::Sub | Op::Mul | Op::Div | Op::Rem | Op::Pow => {
                self.expect(Type::Integer, left, span, "arithmetic operator");
                self.expect(Type::Integer, right, span, "arithmetic operator");
                Type::Integer
            }
            Op::Less | Op::Greater | Op::LessEqual | Op::GreaterEqual => {
                if left == Type::Unknown || right == Type::Unknown {
                    return Type::Boolean;
                }
                if left == right && matches!(left, Type::Integer | Type::String) {
                    Type::Boolean
                } else {
                    self.binary_error(operator, left, right, span);
                    Type::Boolean
                }
            }
            Op::Equal | Op::NotEqual => {
                if left != Type::Unknown && right != Type::Unknown && left != right {
                    self.binary_error(operator, left, right, span);
                }
                Type::Boolean
            }
            Op::And | Op::Or => {
                self.expect(Type::Boolean, left, span, "logical operator");
                self.expect(Type::Boolean, right, span, "logical operator");
                Type::Boolean
            }
        }
    }

    fn expect(&mut self, expected: Type, actual: Type, span: Span, context: &str) {
        if expected != Type::Unknown && actual != Type::Unknown && expected != actual {
            self.error(
                span,
                format!("type error: {context} expects `{expected}`, found `{actual}`"),
            );
        }
    }

    fn expect_argument(&mut self, expected: Type, actual: &[Type], index: usize, name: &str) {
        if let Some(actual) = actual.get(index).copied() {
            self.expect(
                expected,
                actual,
                Span::new(0, 0),
                &format!("`{name}` argument {}", index + 1),
            );
        }
    }

    fn expect_one_of(&mut self, actual: &[Type], index: usize, allowed: &[Type], name: &str) {
        if let Some(actual) = actual.get(index).copied()
            && actual != Type::Unknown
            && !allowed.contains(&actual)
        {
            let names = allowed
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" or ");
            self.error(
                Span::new(0, 0),
                format!("type error: `{name}` expects `{names}`, found `{actual}`"),
            );
        }
    }

    fn binary_error(&mut self, operator: BinaryOperator, left: Type, right: Type, span: Span) {
        self.error(
            span,
            format!("type error: operator `{operator}` cannot combine `{left}` and `{right}`"),
        );
    }

    fn name(&self, span: Span, description: &str) -> String {
        self.source.slice(span).unwrap_or(description).to_owned()
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.sink.emit(Diagnostic::error(message).at(span));
    }
}

/// Returns whether a function body ends with an explicit return statement.
fn ends_with_return(body: &AstNode) -> bool {
    let AstKind::Block { statements } = &body.kind else {
        return false;
    };
    matches!(
        statements.last().map(|statement| &statement.kind),
        Some(AstKind::Return { .. })
    )
}

fn builtin(name: &str) -> Option<BuiltinFunction> {
    BuiltinFunction::all().find(|builtin| builtin.name() == name)
}

/// Returns whether a syntax tree contains any optional v2 annotation.
///
/// This uses an explicit stack so it cannot compromise the evaluator's
/// support for long, left-associated binary expressions.
fn requires_check(root: &AstNode) -> bool {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        match &node.kind {
            AstKind::Let {
                annotation, value, ..
            } => {
                if annotation.is_some() {
                    return true;
                }
                pending.push(value);
            }
            AstKind::Function {
                parameters,
                return_type,
                body,
                ..
            } => {
                if return_type.is_some()
                    || parameters
                        .iter()
                        .any(|parameter| parameter.annotation.is_some())
                {
                    return true;
                }
                pending.push(body);
            }
            AstKind::Program { statements } | AstKind::Block { statements } => {
                pending.extend(statements)
            }
            AstKind::List { elements } => pending.extend(elements),
            AstKind::Member { object, .. }
            | AstKind::Group { expression: object }
            | AstKind::Unary {
                operand: object, ..
            } => pending.push(object),
            AstKind::Index { object, index } => {
                pending.push(object);
                pending.push(index);
            }
            AstKind::Binary { left, right, .. }
            | AstKind::Assignment {
                target: left,
                value: right,
            } => {
                pending.push(left);
                pending.push(right);
            }
            AstKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                pending.push(condition);
                pending.push(then_branch);
                if let Some(branch) = else_branch {
                    pending.push(branch);
                }
            }
            AstKind::While { condition, body } => {
                pending.push(condition);
                pending.push(body);
            }
            AstKind::For {
                start, end, body, ..
            } => {
                if let Some(start) = start {
                    pending.push(start);
                }
                pending.push(end);
                pending.push(body);
            }
            AstKind::Call { callee, arguments } => {
                pending.push(callee);
                pending.extend(arguments);
            }
            AstKind::Return { value } => {
                if let Some(value) = value {
                    pending.push(value);
                }
            }
            AstKind::Integer
            | AstKind::BooleanLiteral(_)
            | AstKind::StringLiteral
            | AstKind::Identifier
            | AstKind::Break
            | AstKind::Continue
            | AstKind::Use { .. } => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn check_source(contents: &str, strict: bool) -> DiagnosticSink {
        let source = SourceFile::new("types.ucl", contents);
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);
        let ast = Parser::new(tokens).parse(&mut sink).expect("parses");
        let mut context = TypeContext::new();
        let _ = check(&ast, &source, &mut context, &mut sink, strict);
        sink
    }

    #[test]
    fn unannotated_runtime_mismatches_remain_dynamic() {
        let sink = check_source("let x = 1; x + true;", false);
        assert!(!sink.has_errors(), "{:?}", sink.iter().collect::<Vec<_>>());
    }

    #[test]
    fn annotated_initializer_is_checked() {
        let sink = check_source("let x: int = true;", false);
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("initializer expects `int`"))
        );
    }

    #[test]
    fn typed_function_checks_operator_and_call_arguments() {
        let sink = check_source("fn twice(x: int): int { x + true; }; twice(\"no\");", false);
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("operator `+`"))
        );
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("function argument expects `int`")
        }));
    }

    #[test]
    fn persistent_context_protects_a_prior_typed_binding() {
        let source = SourceFile::new("session.ucl", "let x: int = 1;");
        let mut sink = DiagnosticSink::new();
        let ast = Parser::new(Lexer::new(&source).tokenize(&mut sink))
            .parse(&mut sink)
            .expect("parses");
        let mut context = TypeContext::new();
        assert!(check(&ast, &source, &mut context, &mut sink, false));

        let next_source = SourceFile::new("session.ucl", "x = true;");
        let mut next_sink = DiagnosticSink::new();
        let next_ast = Parser::new(Lexer::new(&next_source).tokenize(&mut next_sink))
            .parse(&mut next_sink)
            .expect("parses");
        assert!(!check(
            &next_ast,
            &next_source,
            &mut context,
            &mut next_sink,
            false,
        ));
        assert!(
            next_sink
                .iter()
                .any(|diagnostic| diagnostic.message.contains("assignment expects `int`"))
        );
    }

    #[test]
    fn strict_mode_rejects_unannotated_functions() {
        let sink = check_source("fn identity(x) { x; };", true);
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("--strict-types"))
        );
    }
}
