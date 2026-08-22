//! Evaluator: executes an abstract syntax tree.
//!
//! The evaluator walks the AST produced by the [`Parser`](crate::parser::Parser)
//! and computes a [`Value`]. The [`SourceFile`] is needed alongside the AST
//! because nodes carry [`Span`]s rather than their text, so identifier names
//! and integer literals are read back out of the source.
//!
//! Operator semantics, until the specification is written:
//!
//! - Integers: `+`, `-`, `*`, `/`, `%`, and `^` (exponentiation).
//! - Comparison: `<` and `>` on integers, producing booleans.
//! - Booleans: unary `!`, and infix `&` (and) / `|` (or).
//!
//! ## Error Handling
//!
//! The evaluator follows a "collect all errors" strategy:
//! - Errors are emitted to the `DiagnosticSink` but do not stop execution
//! - Evaluation continues to report multiple errors in a single pass
//! - [`Evaluator::evaluate`] returns `None` when any runtime error occurred,
//!   and `Some(value)` with the program's value otherwise
//!
//! This design allows users to see all errors in their program at once,
//! similar to how compilers like rustc operate.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use crate::diagnostic::{Diagnostic, DiagnosticSink, Severity};
use crate::lexer::unescape_string;
use crate::parser::{AstKind, AstNode, BinaryOperator};
use crate::source::{SourceFile, Span};

/// The maximum evaluation-nesting depth allowed before an error is reported.
///
/// This guards against deeply nested ASTs constructed through the public API
/// that would otherwise overflow the call stack during recursive evaluation.
/// Binary operator chains are flattened into loops (see `eval`), so recursion
/// depth tracks expression nesting: any program the parser accepts at its
/// limit of 256 nested expressions stays well below this bound, and a
/// program the parser accepts must never be rejected here.
const MAX_EVAL_DEPTH: usize = 512;

/// The maximum number of iterations a single `while` loop may run.
///
/// Without a bound, `while true { }` would hang the interpreter forever.
/// The cap also keeps fuzzing and long-lived embedding processes safe from
/// runaway loops; legitimate programs rarely approach it.
const MAX_LOOP_ITERATIONS: u64 = 100_000;

/// The maximum number of active UCL function calls.
///
/// This prevents recursive programs from exhausting the host call stack.
const MAX_CALL_DEPTH: usize = 64;

/// A runtime value produced by evaluating a program.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// No value (unit).
    ///
    /// This is the result of declarations and other statements that don't
    /// produce a value.
    Unit,
    /// A signed 64-bit integer.
    Integer(i64),
    /// A boolean value (`true` or `false`).
    Boolean(bool),
    /// A string of Unicode text.
    Str(String),
    /// A named UCL function that can be called with positional arguments.
    Function(FunctionValue),
}

/// A callable UCL function value.
///
/// Functions declared by v0.4 execute with the global scope and a fresh
/// parameter scope, so calls are not dynamically scoped through their callers.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionValue {
    parameters: Vec<String>,
    body: AstNode,
}

impl Value {
    /// Returns the value's short type name, used in diagnostics.
    fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "unit",
            Value::Integer(_) => "integer",
            Value::Boolean(_) => "boolean",
            Value::Str(_) => "string",
            Value::Function(_) => "function",
        }
    }
}

/// A stack of lexical scopes, each mapping names to values.
///
/// Lookups walk the stack from the innermost scope outward, so a block can
/// shadow an outer binding while leaving the outer binding intact.
///
/// This struct is not exposed publicly; it is an internal implementation detail
/// of the [`Evaluator`].
#[derive(Default)]
struct Environment {
    /// The stack of scopes, with the innermost (most recent) scope at the end.
    scopes: Vec<HashMap<String, Value>>,
}

impl Environment {
    /// Pushes a new, empty scope onto the stack.
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pops the innermost scope from the stack.
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Binds `name` to `value` in the innermost scope.
    ///
    /// If a binding with the same name already exists in the innermost scope,
    /// it is shadowed (replaced) by the new binding.
    fn define(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("the environment always has at least one scope")
            .insert(name.to_owned(), value);
    }

    /// Looks up `name`, searching scopes from innermost outward.
    ///
    /// Returns the first binding found, or `None` if no binding exists.
    fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    /// Returns whether evaluation is currently in the program's global scope.
    fn is_global(&self) -> bool {
        self.scopes.len() == 1
    }

    /// Begins a function call with the global scope and a fresh parameter
    /// scope, returning the caller's scopes for restoration when it finishes.
    fn begin_call(&mut self) -> Vec<HashMap<String, Value>> {
        let mut caller_scopes = std::mem::take(&mut self.scopes);
        let global = caller_scopes.remove(0);
        self.scopes.push(global);
        self.push_scope();
        caller_scopes
    }

    /// Restores caller scopes after a function call while preserving changes
    /// the function made to global bindings.
    fn end_call(&mut self, mut caller_scopes: Vec<HashMap<String, Value>>) {
        let global = self.scopes.remove(0);
        caller_scopes.insert(0, global);
        self.scopes = caller_scopes;
    }

    /// Reassigns an existing binding. Returns `false` if `name` is unbound.
    ///
    /// The assignment searches scopes from innermost to outermost and updates
    /// the first binding found. This allows shadowed bindings to be updated
    /// correctly.
    fn assign(&mut self, name: &str, value: Value) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                *slot = value;
                return true;
            }
        }
        false
    }
}

/// Walks an [`AstNode`] and computes a [`Value`].
///
/// The evaluator implements the semantic rules of the UCL language,
/// executing the abstract syntax tree to produce runtime values.
pub struct Evaluator {
    /// Number of UCL calls currently active during evaluation.
    call_depth: Cell<usize>,
}

impl Evaluator {
    /// Creates a new evaluator instance.
    ///
    /// The evaluator may be reused to evaluate multiple ASTs; each evaluation
    /// resets its active function-call counter.
    pub fn new() -> Self {
        Self {
            call_depth: Cell::new(0),
        }
    }

    /// Evaluates the given AST to produce a value.
    ///
    /// `source` provides the text that the AST's spans point into. On
    /// success, the result is the value of the program's last statement, or
    /// [`Value::Unit`] if the program is empty.
    ///
    /// # Error Handling
    ///
    /// Returns `None` if any runtime error was emitted to `sink` during this
    /// call (diagnostics already present in the sink before the call are not
    /// considered), and `Some(value)` otherwise. This makes failure explicit:
    /// callers never need to guess whether a [`Value::Unit`] result means a
    /// real unit value or an error.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let source = SourceFile::new("example.ucl", "2 + 3 * 4");
    /// let mut sink = DiagnosticSink::new();
    /// let ast = /* parse the source */;
    /// match Evaluator::new().evaluate(&ast, &source, &mut sink) {
    ///     Some(value) => println!("{value:?}"),
    ///     None => { /* handle runtime errors */ }
    /// }
    /// ```
    pub fn evaluate(
        &self,
        root: &AstNode,
        source: &SourceFile,
        sink: &mut DiagnosticSink,
    ) -> Option<Value> {
        let mut environment = Environment::default();
        environment.push_scope();
        self.call_depth.set(0);
        let baseline = sink.len();
        let value = self.eval(root, source, &mut environment, sink, 0);
        let failed = sink
            .iter()
            .skip(baseline)
            .any(|d| d.severity == Severity::Error);
        if failed { None } else { Some(value) }
    }

    /// Internal evaluation function that walks the AST.
    ///
    /// This recursively evaluates nodes and returns their values.
    /// Errors are emitted to the sink but do not stop evaluation.
    fn eval(
        &self,
        node: &AstNode,
        source: &SourceFile,
        environment: &mut Environment,
        sink: &mut DiagnosticSink,
        depth: usize,
    ) -> Value {
        if depth >= MAX_EVAL_DEPTH {
            sink.emit(Diagnostic::error("evaluation nesting is too deep").at(node.span));
            return Value::Unit;
        }

        match &node.kind {
            AstKind::Program { statements } => {
                let mut last = Value::Unit;
                for statement in statements {
                    last = self.eval(statement, source, environment, sink, depth + 1);
                }
                last
            }
            AstKind::Block { statements } => {
                environment.push_scope();
                let mut last = Value::Unit;
                for statement in statements {
                    last = self.eval(statement, source, environment, sink, depth + 1);
                }
                environment.pop_scope();
                last
            }
            AstKind::Let { name, value, .. } => {
                // Spans come from the parser and are valid there, but ASTs
                // can also be constructed through the public API with
                // arbitrary spans; report those instead of panicking.
                let Some(name) = source.slice(*name) else {
                    sink.emit(
                        Diagnostic::error("declaration has an invalid name span").at(node.span),
                    );
                    return Value::Unit;
                };
                let name = name.to_owned();
                let value = self.eval(value, source, environment, sink, depth + 1);
                environment.define(&name, value);
                Value::Unit
            }
            AstKind::Identifier => {
                let name = match source.slice(node.span) {
                    Some(name) => name,
                    None => {
                        sink.emit(Diagnostic::error("invalid identifier span").at(node.span));
                        return Value::Unit;
                    }
                };
                match environment.lookup(name) {
                    Some(value) => value.clone(),
                    None => {
                        sink.emit(
                            Diagnostic::error(format!("undefined variable `{name}`")).at(node.span),
                        );
                        Value::Unit
                    }
                }
            }
            AstKind::Integer => {
                let text = match source.slice(node.span) {
                    Some(text) => text,
                    None => {
                        sink.emit(Diagnostic::error("invalid integer literal span").at(node.span));
                        return Value::Unit;
                    }
                };
                let text = text.to_owned();
                match text.parse::<i64>() {
                    Ok(value) => Value::Integer(value),
                    Err(_) => {
                        sink.emit(
                            Diagnostic::error(format!("integer literal `{text}` is out of range"))
                                .at(node.span),
                        );
                        Value::Integer(0)
                    }
                }
            }
            AstKind::BooleanLiteral(value) => Value::Boolean(*value),
            AstKind::StringLiteral => {
                let text = match source.slice(node.span) {
                    Some(text) => text,
                    None => {
                        sink.emit(Diagnostic::error("invalid string literal span").at(node.span));
                        return Value::Unit;
                    }
                };
                Value::Str(unescape_string(text))
            }
            AstKind::Group { expression } => {
                self.eval(expression, source, environment, sink, depth + 1)
            }
            AstKind::Unary { operator, operand } => {
                let operand = self.eval(operand, source, environment, sink, depth + 1);
                self.eval_unary(*operator, operand, node.span, sink)
            }
            AstKind::Binary {
                operator,
                left,
                right,
            } => {
                // A flat, left-associative chain such as `1 + 1 + ...` adds
                // one evaluator level per link without adding any parser
                // nesting, so recursing per link would reject arbitrarily
                // long chains the parser accepts. Flattening the left spine
                // into an explicit loop keeps recursion depth bounded by
                // expression nesting instead of chain length. Operands are
                // still evaluated strictly left to right.
                let mut operators = vec![*operator];
                let mut operands = vec![right];
                let mut spans = vec![node.span];
                let mut current: &AstNode = left;
                while let AstKind::Binary {
                    operator: nested_operator,
                    left: nested_left,
                    right: nested_right,
                } = &current.kind
                {
                    operators.push(*nested_operator);
                    operands.push(nested_right);
                    spans.push(current.span);
                    current = nested_left;
                }

                let mut accumulator = self.eval(current, source, environment, sink, depth + 1);
                for index in (0..operators.len()).rev() {
                    let operator = operators[index];
                    // Short-circuiting: `&` skips its right-hand side once the
                    // left is false and `|` skips it once the left is true.
                    if operator == BinaryOperator::And && accumulator == Value::Boolean(false) {
                        continue;
                    }
                    if operator == BinaryOperator::Or && accumulator == Value::Boolean(true) {
                        continue;
                    }
                    let operand = self.eval(operands[index], source, environment, sink, depth + 1);
                    accumulator =
                        self.eval_binary(operator, accumulator, operand, spans[index], sink);
                }
                accumulator
            }
            AstKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition_value = self.eval(condition, source, environment, sink, depth + 1);
                match condition_value {
                    Value::Boolean(true) => {
                        self.eval(then_branch, source, environment, sink, depth + 1)
                    }
                    Value::Boolean(false) => match else_branch {
                        Some(branch) => self.eval(branch, source, environment, sink, depth + 1),
                        None => Value::Unit,
                    },
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "condition of `if` must be a boolean, found `{}`",
                                other.type_name()
                            ))
                            .at(condition.span),
                        );
                        Value::Unit
                    }
                }
            }
            AstKind::Function {
                name,
                parameters,
                body,
                ..
            } => {
                if !environment.is_global() {
                    sink.emit(
                        Diagnostic::error(
                            "function declarations are only allowed at program scope",
                        )
                        .at(node.span),
                    );
                    return Value::Unit;
                }

                let Some(name) = source.slice(*name) else {
                    sink.emit(
                        Diagnostic::error("function declaration has an invalid name span")
                            .at(node.span),
                    );
                    return Value::Unit;
                };
                let mut names = Vec::with_capacity(parameters.len());
                let mut seen = HashSet::new();
                for parameter_span in parameters {
                    let Some(parameter) = source.slice(*parameter_span) else {
                        sink.emit(
                            Diagnostic::error("function declaration has an invalid parameter span")
                                .at(node.span),
                        );
                        return Value::Unit;
                    };
                    if !seen.insert(parameter) {
                        sink.emit(
                            Diagnostic::error(format!(
                                "duplicate function parameter `{parameter}`"
                            ))
                            .at(*parameter_span),
                        );
                        return Value::Unit;
                    }
                    names.push(parameter.to_owned());
                }

                environment.define(
                    name,
                    Value::Function(FunctionValue {
                        parameters: names,
                        body: (**body).clone(),
                    }),
                );
                Value::Unit
            }
            AstKind::Call { callee, arguments } => {
                let callable = self.eval(callee, source, environment, sink, depth + 1);
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(self.eval(argument, source, environment, sink, depth + 1));
                }

                let Value::Function(function) = callable else {
                    sink.emit(
                        Diagnostic::error(format!(
                            "cannot call value of type `{}`",
                            callable.type_name()
                        ))
                        .at(callee.span),
                    );
                    return Value::Unit;
                };
                if function.parameters.len() != values.len() {
                    sink.emit(
                        Diagnostic::error(format!(
                            "function expected {} argument(s), received {}",
                            function.parameters.len(),
                            values.len()
                        ))
                        .at(node.span),
                    );
                    return Value::Unit;
                }
                let call_depth = self.call_depth.get();
                if call_depth >= MAX_CALL_DEPTH {
                    sink.emit(Diagnostic::error("function call depth is too deep").at(node.span));
                    return Value::Unit;
                }

                self.call_depth.set(call_depth + 1);
                let caller_scopes = environment.begin_call();
                for (parameter, value) in function.parameters.iter().zip(values) {
                    environment.define(parameter, value);
                }
                let value = self.eval(&function.body, source, environment, sink, depth + 1);
                environment.end_call(caller_scopes);
                self.call_depth.set(call_depth);
                value
            }
            AstKind::While { condition, body } => {
                let mut iterations = 0u64;
                loop {
                    iterations += 1;
                    if iterations > MAX_LOOP_ITERATIONS {
                        sink.emit(
                            Diagnostic::error("loop exceeded the maximum number of iterations")
                                .at(node.span),
                        );
                        break;
                    }
                    let condition_value =
                        self.eval(condition, source, environment, sink, depth + 1);
                    match condition_value {
                        Value::Boolean(true) => {
                            self.eval(body, source, environment, sink, depth + 1);
                        }
                        Value::Boolean(false) => break,
                        other => {
                            sink.emit(
                                Diagnostic::error(format!(
                                    "condition of `while` must be a boolean, found `{}`",
                                    other.type_name()
                                ))
                                .at(condition.span),
                            );
                            break;
                        }
                    }
                }
                Value::Unit
            }
            AstKind::Assignment { target, value } => {
                let name = match &target.kind {
                    AstKind::Identifier => match source.slice(target.span) {
                        Some(name) => name.to_owned(),
                        None => {
                            sink.emit(
                                Diagnostic::error("invalid assignment target span").at(target.span),
                            );
                            return Value::Unit;
                        }
                    },
                    _ => {
                        sink.emit(Diagnostic::error("invalid assignment target").at(target.span));
                        return Value::Unit;
                    }
                };
                let value = self.eval(value, source, environment, sink, depth + 1);
                if !environment.assign(&name, value.clone()) {
                    sink.emit(
                        Diagnostic::error(format!("cannot assign to undefined variable `{name}`"))
                            .at(target.span),
                    );
                }
                value
            }
        }
    }

    fn eval_unary(
        &self,
        operator: char,
        operand: Value,
        span: Span,
        sink: &mut DiagnosticSink,
    ) -> Value {
        match (&operand, operator) {
            (Value::Integer(value), '+') => Value::Integer(*value),
            (Value::Integer(value), '-') => match value.checked_neg() {
                Some(negated) => Value::Integer(negated),
                None => overflow(span, sink),
            },
            (Value::Boolean(value), '!') => Value::Boolean(!value),
            _ => unary_type_error(operator, &operand, span, sink),
        }
    }

    fn eval_binary(
        &self,
        operator: BinaryOperator,
        left: Value,
        right: Value,
        span: Span,
        sink: &mut DiagnosticSink,
    ) -> Value {
        use BinaryOperator::*;
        match operator {
            Add => {
                // `+` is overloaded: integer addition or string concatenation.
                match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => match a.checked_add(b) {
                        Some(sum) => Value::Integer(sum),
                        None => overflow(span, sink),
                    },
                    (Value::Str(a), Value::Str(b)) => {
                        let mut concatenated = a;
                        concatenated.push_str(&b);
                        Value::Str(concatenated)
                    }
                    (l, r) => binary_type_error(operator, &l, &r, span, sink),
                }
            }
            Sub | Mul | Div | Rem | Pow => {
                let (a, b) = match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => (a, b),
                    (l, r) => return binary_type_error(operator, &l, &r, span, sink),
                };
                match operator {
                    Sub => match a.checked_sub(b) {
                        Some(difference) => Value::Integer(difference),
                        None => overflow(span, sink),
                    },
                    Mul => match a.checked_mul(b) {
                        Some(product) => Value::Integer(product),
                        None => overflow(span, sink),
                    },
                    Div if b == 0 => division_by_zero(span, sink),
                    Div => match a.checked_div(b) {
                        Some(quotient) => Value::Integer(quotient),
                        None => overflow(span, sink),
                    },
                    Rem if b == 0 => division_by_zero(span, sink),
                    Rem => match a.checked_rem(b) {
                        Some(remainder) => Value::Integer(remainder),
                        None => overflow(span, sink),
                    },
                    Pow if b < 0 => {
                        sink.emit(
                            Diagnostic::error("negative exponents are not supported").at(span),
                        );
                        Value::Unit
                    }
                    Pow => {
                        let exponent = match u32::try_from(b) {
                            Ok(exponent) => exponent,
                            // The exponent exceeds `u32::MAX`. Only the bases
                            // 0, 1, and -1 can produce an in-range result from
                            // such an exponent; every other base overflows the
                            // signed 64-bit range.
                            Err(_) => {
                                return match a {
                                    0 => Value::Integer(0),
                                    1 => Value::Integer(1),
                                    -1 => Value::Integer(if b % 2 == 0 { 1 } else { -1 }),
                                    _ => overflow(span, sink),
                                };
                            }
                        };
                        match a.checked_pow(exponent) {
                            Some(power) => Value::Integer(power),
                            None => overflow(span, sink),
                        }
                    }
                    _ => unreachable!("`operator` is restricted to arithmetic operators"),
                }
            }
            Less | Greater | LessEqual | GreaterEqual => {
                let (a, b) = match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => (a, b),
                    (l, r) => return binary_type_error(operator, &l, &r, span, sink),
                };
                Value::Boolean(match operator {
                    Less => a < b,
                    Greater => a > b,
                    LessEqual => a <= b,
                    GreaterEqual => a >= b,
                    _ => unreachable!("`operator` is restricted to relational operators"),
                })
            }
            Equal | NotEqual => {
                // Equality is defined for two integers, two booleans, or two
                // strings; comparing values of different types is an error.
                match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => {
                        Value::Boolean(if operator == Equal { a == b } else { a != b })
                    }
                    (Value::Boolean(a), Value::Boolean(b)) => {
                        Value::Boolean(if operator == Equal { a == b } else { a != b })
                    }
                    (Value::Str(a), Value::Str(b)) => {
                        Value::Boolean(if operator == Equal { a == b } else { a != b })
                    }
                    (l, r) => binary_type_error(operator, &l, &r, span, sink),
                }
            }
            And | Or => {
                // Reached only when short-circuiting did not skip the
                // right-hand side (see the `Binary` arm in `eval`).
                let (a, b) = match (left, right) {
                    (Value::Boolean(a), Value::Boolean(b)) => (a, b),
                    (l, r) => return binary_type_error(operator, &l, &r, span, sink),
                };
                Value::Boolean(if operator == And { a && b } else { a || b })
            }
        }
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// Emits a type error for a unary operator and returns [`Value::Unit`].
///
/// This is used when a unary operator is applied to an operand of the wrong type.
fn unary_type_error(
    operator: char,
    operand: &Value,
    span: Span,
    sink: &mut DiagnosticSink,
) -> Value {
    sink.emit(
        Diagnostic::error(format!(
            "cannot apply `{operator}` to `{}`",
            operand.type_name()
        ))
        .at(span),
    );
    Value::Unit
}

/// Emits a type error for a binary operator and returns [`Value::Unit`].
///
/// This is used when a binary operator is applied to operands of the wrong types.
fn binary_type_error(
    operator: BinaryOperator,
    left: &Value,
    right: &Value,
    span: Span,
    sink: &mut DiagnosticSink,
) -> Value {
    sink.emit(
        Diagnostic::error(format!(
            "cannot apply `{operator}` to `{}` and `{}`",
            left.type_name(),
            right.type_name()
        ))
        .at(span),
    );
    Value::Unit
}

/// Emits a division by zero error and returns [`Value::Unit`].
fn division_by_zero(span: Span, sink: &mut DiagnosticSink) -> Value {
    sink.emit(Diagnostic::error("division by zero").at(span));
    Value::Unit
}

/// Emits an integer overflow error and returns [`Value::Unit`].
fn overflow(span: Span, sink: &mut DiagnosticSink) -> Value {
    sink.emit(Diagnostic::error("integer overflow").at(span));
    Value::Unit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn eval(source_text: &str) -> (Option<Value>, DiagnosticSink) {
        let source = SourceFile::new("test.ucl", source_text);
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);
        let ast = Parser::new(tokens)
            .parse(&mut sink)
            .expect("parser should return a program");
        let value = Evaluator::new().evaluate(&ast, &source, &mut sink);
        (value, sink)
    }

    #[test]
    fn evaluates_integer_literals() {
        let (value, sink) = eval("42;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn respects_operator_precedence() {
        let (value, sink) = eval("2 + 3 * 4;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(14)));
    }

    #[test]
    fn exponentiation_binds_tighter_than_multiplication() {
        let (value, sink) = eval("2 ^ 3 * 2;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(16)));
    }

    #[test]
    fn evaluates_bindings_and_references() {
        let (value, sink) = eval("let x = 5; x + 1;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(6)));
    }

    #[test]
    fn assignment_updates_existing_bindings() {
        let (value, sink) = eval("let x = 5; x = 10; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(10)));
    }

    #[test]
    fn blocks_introduce_new_scopes() {
        let (value, sink) = eval("let x = 5; { let x = 10; }; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(5)));
    }

    #[test]
    fn comparisons_and_logic_produce_booleans() {
        let (value, sink) = eval("1 < 2;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));

        let (value, sink) = eval("!(1 < 2);");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(false)));

        let (value, sink) = eval("1 < 2 & 2 < 3;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));
    }

    #[test]
    fn reports_undefined_variables() {
        let (_value, sink) = eval("x;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("undefined variable `x`"))
        );
    }

    #[test]
    fn reports_division_by_zero() {
        let (_value, sink) = eval("1 / 0;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("division by zero"))
        );
    }

    #[test]
    fn rejects_exponents_above_the_u32_range() {
        // These exponents overflow i64, but they must be reported as overflow
        // rather than silently truncated (`4294967296 as u32 == 0`), which
        // would wrongly evaluate `2 ^ 4294967296` as `2 ^ 0 == 1`.
        for source in ["2 ^ 4294967296;", "2 ^ 4294967297;"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("overflow")),
                "expected an overflow error for `{source}`"
            );
        }
    }

    #[test]
    fn rejects_excessive_evaluation_nesting() {
        // Build a deeply nested AST directly, bypassing the parser's own depth
        // limit, to exercise the evaluator's independent guard. The test runs
        // on a thread with a generous stack so that reaching the guard never
        // depends on how large one evaluator frame happens to be.
        let harness = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let mut node = AstNode {
                    span: Span::new(0, 1),
                    kind: AstKind::Integer,
                };
                for _ in 0..(MAX_EVAL_DEPTH + 100) {
                    node = AstNode {
                        span: Span::new(0, 1),
                        kind: AstKind::Group {
                            expression: Box::new(node),
                        },
                    };
                }
                let program = AstNode {
                    span: Span::new(0, 1),
                    kind: AstKind::Program {
                        statements: vec![node],
                    },
                };
                let source = SourceFile::new("test.ucl", "1");
                let mut sink = DiagnosticSink::new();

                let _ = Evaluator::new().evaluate(&program, &source, &mut sink);

                sink
            })
            .expect("test harness thread spawns");
        let sink = harness.join().expect("harness thread does not panic");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("nesting is too deep"))
        );
    }

    #[test]
    fn computes_unit_bases_with_oversized_exponents() {
        // Bases whose powers never overflow produce exact results even when
        // the exponent is too large to represent as a `u32`.
        let (value, sink) = eval("0 ^ 4294967296;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(0)));

        let (value, sink) = eval("1 ^ 4294967296;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(1)));

        let (value, sink) = eval("-1 ^ 4294967296;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(1)));

        let (value, sink) = eval("-1 ^ 4294967297;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(-1)));
    }

    #[test]
    fn reports_overflow_on_addition() {
        let (_value, sink) = eval("9223372036854775807 + 1;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("integer overflow"))
        );
    }

    #[test]
    fn reports_overflow_on_subtraction() {
        let (_value, sink) = eval("-9223372036854775807 - 2;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("integer overflow"))
        );
    }

    #[test]
    fn reports_overflow_on_multiplication() {
        let (_value, sink) = eval("9223372036854775807 * 2;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("integer overflow"))
        );
    }

    #[test]
    fn reports_remainder_by_zero() {
        let (_value, sink) = eval("5 % 0;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("division by zero"))
        );
    }

    #[test]
    fn reports_negative_exponents() {
        let (_value, sink) = eval("2 ^ -1;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("negative exponents"))
        );
    }

    #[test]
    fn evaluates_equality_on_integers_and_booleans() {
        for (source, expected) in [
            ("1 == 1;", true),
            ("1 == 2;", false),
            ("1 != 2;", true),
            ("true == true;", true),
            ("false != true;", true),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
        }
    }

    #[test]
    fn rejects_equality_across_types() {
        let (_value, sink) = eval("1 == true;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("cannot apply `==`"))
        );
    }

    #[test]
    fn evaluates_relational_operators() {
        for (source, expected) in [
            ("1 <= 1;", true),
            ("1 < 1;", false),
            ("2 >= 1;", true),
            ("2 > 3;", false),
            ("1 < 2 == true;", true),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
        }
    }

    #[test]
    fn logical_operators_short_circuit() {
        // The right-hand side would raise a division-by-zero error, but
        // short-circuiting must skip it entirely.
        let (value, sink) = eval("false & 1 / 0;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(false)));

        let (value, sink) = eval("true | 1 / 0;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));

        // Short-circuiting also skips undefined-variable errors.
        let (value, sink) = eval("false & missing;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(false)));
    }

    #[test]
    fn short_circuiting_does_not_skip_needed_sides() {
        // The left-hand side is evaluated even when the right could be
        // skipped: `1 / 0` must still report its error.
        let (_value, sink) = eval("1 / 0 & true;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("division by zero"))
        );

        // When the left-hand side does not force a skip, the right-hand side
        // is still evaluated.
        let (value, sink) = eval("true & (1 / 0) == 0 | true;");
        assert!(sink.has_errors(), "expected the right side to be evaluated");
        let _ = value;
    }

    #[test]
    fn evaluates_boolean_literals() {
        let (value, sink) = eval("true;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));

        let (value, sink) = eval("false;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(false)));

        let (value, sink) = eval("!false;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));
    }

    #[test]
    fn reports_unary_type_errors() {
        for source in ["-(1 < 2);", "!5;"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("cannot apply")),
                "expected a type error for `{source}`"
            );
        }
    }

    #[test]
    fn reports_binary_type_errors() {
        for source in ["1 + (1 < 2);", "1 & 2;", "1 < 2 < 3;"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("cannot apply")),
                "expected a type error for `{source}`"
            );
        }
    }

    #[test]
    fn reports_invalid_assignment_targets() {
        let (_value, sink) = eval("(x) = 5;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("invalid assignment target"))
        );
    }

    #[test]
    fn reports_out_of_range_integer_literals() {
        let (_value, sink) = eval("9223372036854775808;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("out of range"))
        );
    }

    #[test]
    fn evaluates_logical_or_on_booleans() {
        let (value, sink) = eval("1 > 2 | 2 < 3;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));
    }

    #[test]
    fn assignment_inside_a_block_updates_the_outer_binding() {
        let (value, sink) = eval("let x = 5; { x = 10; }; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(10)));
    }

    #[test]
    fn assignment_to_a_shadowed_binding_stays_in_its_scope() {
        let (value, sink) = eval("let x = 5; { let x = 10; x = 20; }; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(5)));
    }

    #[test]
    fn reports_invalid_spans_from_hand_built_asts_instead_of_panicking() {
        // AST nodes are publicly constructible, so spans may point outside
        // the source. The evaluator must report a diagnostic rather than
        // panic when reading names or literals through such a span.
        let source = SourceFile::new("test.ucl", "let x = 5;");
        let program = AstNode {
            span: Span::new(0, 10),
            kind: AstKind::Program {
                statements: vec![
                    AstNode {
                        span: Span::new(100, 200),
                        kind: AstKind::Identifier,
                    },
                    AstNode {
                        span: Span::new(300, 400),
                        kind: AstKind::Integer,
                    },
                ],
            },
        };
        let mut sink = DiagnosticSink::new();

        let _ = Evaluator::new().evaluate(&program, &source, &mut sink);

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("invalid identifier span"))
        );
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("invalid integer literal span"))
        );
    }

    #[test]
    fn evaluates_moderately_long_flat_binary_chains() {
        let source_text = vec!["1"; 100].join(" + ") + ";";
        let (value, sink) = eval(&source_text);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(100)));
    }

    #[test]
    fn evaluates_long_flat_binary_chains_within_the_parser_limit() {
        // A flat, left-associative chain adds evaluator depth without adding
        // parser nesting. Left-spine flattening keeps such chains iterative,
        // so anything within the parser's limit must evaluate without error.
        let terms = 2_000;
        let source_text = vec!["1"; terms].join(" + ") + ";";
        let (value, sink) = eval(&source_text);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(terms as i64)));
    }

    #[test]
    fn evaluates_string_literals_and_concatenation() {
        let (value, sink) = eval("\"hello\" + \" \" + \"world\";");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("hello world".to_owned())));
    }

    #[test]
    fn decodes_escape_sequences_in_strings() {
        let (value, sink) = eval(r##""a\tb\nc\"d\\";"##);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("a\tb\nc\"d\\".to_owned())));
    }

    #[test]
    fn compares_strings_for_equality() {
        for (source, expected) in [
            ("\"a\" == \"a\";", true),
            ("\"a\" == \"b\";", false),
            ("\"a\" != \"b\";", true),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
        }
    }

    #[test]
    fn rejects_mixed_type_operations_on_strings() {
        for source in ["1 + \"a\";", "\"a\" * 2;", "\"a\" == 1;"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("cannot apply")),
                "expected a type error for `{source}`"
            );
        }
    }

    #[test]
    fn evaluates_the_taken_branch_of_an_if() {
        let (value, sink) = eval("if true { 1; } else { 2; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(1)));

        let (value, sink) = eval("if false { 1; } else { 2; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(2)));
    }

    #[test]
    fn an_if_without_else_yields_unit_when_false() {
        let (value, sink) = eval("if false { 1; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Unit));

        let (value, sink) = eval("if true { 1; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(1)));
    }

    #[test]
    fn only_the_condition_is_evaluated_before_branching() {
        // The untaken branch would raise a division-by-zero error.
        let (value, sink) = eval("if false { 1 / 0; } else { 7; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(7)));

        let (value, sink) = eval("if true { 7; } else { 1 / 0; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(7)));
    }

    #[test]
    fn else_if_chains_pick_the_first_true_branch() {
        let source = "let x = 2; if x == 1 { 10; } else if x == 2 { 20; } else { 30; };";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(20)));
    }

    #[test]
    fn rejects_a_non_boolean_if_condition() {
        let (_value, sink) = eval("if 1 { 2; };");
        assert!(sink.has_errors());
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("condition of `if` must be a boolean")
        }));
    }

    #[test]
    fn rejects_a_non_boolean_while_condition() {
        let (_value, sink) = eval("while \"x\" { 1; };");
        assert!(sink.has_errors());
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("condition of `while` must be a boolean")
        }));
    }

    #[test]
    fn while_loops_iterate_until_the_condition_becomes_false() {
        // The program's last statement is inside the loop's block, so its
        // value is unit; verify the iteration count via a final reference to
        // a binding the loop mutated.
        let source = "
            let total = 0;
            let i = 1;
            while i <= 5 {
                total = total + i;
                i = i + 1;
            };
        ";
        let (value, sink) = eval(&format!("{source} total;"));
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(15)));
    }

    #[test]
    fn an_infinite_loop_is_capped() {
        let (value, sink) = eval("while true { };");
        assert!(sink.has_errors());
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("exceeded the maximum number of iterations")
        }));
        assert_eq!(value, None);
    }

    #[test]
    fn while_body_runs_in_its_own_scope() {
        // A `let` inside the body must not leak into the outer scope, and
        // assignment must still reach the outer binding.
        let source = "
            let i = 0;
            let seen = 0;
            while i < 3 {
                i = i + 1;
                let inner = i;
                seen = inner;
            };
            seen;
        ";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(3)));
    }

    #[test]
    fn strings_flow_through_variables_and_blocks() {
        let source = "let greeting = \"hi \"; let name = \"ucl\"; greeting + name;";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("hi ucl".to_owned())));
    }

    #[test]
    fn calls_named_functions_with_parameters_and_implicit_results() {
        let source = "fn add(left, right) { left + right; }; add(20, 22);";
        let (value, sink) = eval(source);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn functions_resolve_globals_not_caller_locals() {
        let source = "let value = 10; fn read() { value; }; { let value = 20; read(); };";
        let (value, sink) = eval(source);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(10)));
    }

    #[test]
    fn function_calls_evaluate_arguments_left_to_right() {
        let source = "let state = 0; fn first() { state = 1; state; }; fn second() { state = 2; state; }; fn pick(left, right) { right; }; pick(first(), second()); state;";
        let (value, sink) = eval(source);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(2)));
    }

    #[test]
    fn functions_can_recur() {
        let source =
            "fn factorial(n) { if n <= 1 { 1; } else { n * factorial(n - 1); }; }; factorial(5);";
        let (value, sink) = eval(source);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(120)));
    }

    #[test]
    fn reports_function_call_and_declaration_errors() {
        for (source, expected) in [
            (
                "fn identity(value) { value; }; identity();",
                "expected 1 argument",
            ),
            ("42(1);", "cannot call value of type `integer`"),
            (
                "fn duplicate(value, value) { value; };",
                "duplicate function parameter `value`",
            ),
            ("{ fn inner() { 1; }; };", "only allowed at program scope"),
        ] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains(expected)),
                "expected `{expected}` for `{source}`"
            );
        }
    }

    #[test]
    fn caps_recursive_function_calls() {
        let (_value, sink) = eval("fn recurse() { recurse(); }; recurse();");

        assert!(sink.has_errors());
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("function call depth is too deep")
        }));
    }
}
