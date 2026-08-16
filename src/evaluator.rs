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

use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::parser::{AstKind, AstNode};
use crate::source::{SourceFile, Span};

/// A runtime value produced by evaluating a program.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// No value (unit).
    Unit,
    /// A signed integer.
    Integer(i64),
    /// A boolean.
    Boolean(bool),
    // TODO: strings, functions, and the rest of the type system, once the
    // lexer and parser gain syntax for them.
}

impl Value {
    /// The value's short type name, used in diagnostics.
    fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "unit",
            Value::Integer(_) => "integer",
            Value::Boolean(_) => "boolean",
        }
    }
}

/// A stack of lexical scopes, each mapping names to values.
///
/// Lookups walk the stack from the innermost scope outward, so a block can
/// shadow an outer binding while leaving the outer binding intact.
#[derive(Default)]
struct Environment {
    scopes: Vec<HashMap<String, Value>>,
}

impl Environment {
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Binds `name` to `value` in the innermost scope.
    fn define(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("the environment always has at least one scope")
            .insert(name.to_owned(), value);
    }

    /// Looks up `name`, searching scopes from innermost outward.
    fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    /// Reassigns an existing binding. Returns `false` if `name` is unbound.
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
pub struct Evaluator;

impl Evaluator {
    /// Creates an evaluator.
    pub fn new() -> Self {
        Self
    }

    /// Evaluates `root` to a value, emitting errors into `sink`.
    ///
    /// `source` provides the text that the AST's spans point into. The result
    /// is the value of the program's last statement, or [`Value::Unit`] if the
    /// program is empty or an error occurred.
    pub fn evaluate(
        &self,
        root: &AstNode,
        source: &SourceFile,
        sink: &mut DiagnosticSink,
    ) -> Value {
        let mut environment = Environment::default();
        environment.push_scope();
        self.eval(root, source, &mut environment, sink)
    }

    fn eval(
        &self,
        node: &AstNode,
        source: &SourceFile,
        environment: &mut Environment,
        sink: &mut DiagnosticSink,
    ) -> Value {
        match &node.kind {
            AstKind::Program { statements } => {
                let mut last = Value::Unit;
                for statement in statements {
                    last = self.eval(statement, source, environment, sink);
                }
                last
            }
            AstKind::Block { statements } => {
                environment.push_scope();
                let mut last = Value::Unit;
                for statement in statements {
                    last = self.eval(statement, source, environment, sink);
                }
                environment.pop_scope();
                last
            }
            AstKind::Let { name, value, .. } => {
                let name = lexeme(source, *name);
                let value = self.eval(value, source, environment, sink);
                environment.define(name, value);
                Value::Unit
            }
            AstKind::Identifier => match environment.lookup(lexeme(source, node.span)) {
                Some(value) => value.clone(),
                None => {
                    sink.emit(
                        Diagnostic::error(format!(
                            "undefined variable `{}`",
                            lexeme(source, node.span)
                        ))
                        .at(node.span),
                    );
                    Value::Unit
                }
            },
            AstKind::Integer => {
                let text = lexeme(source, node.span);
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
            AstKind::Group { expression } => self.eval(expression, source, environment, sink),
            AstKind::Unary { operator, operand } => {
                let operand = self.eval(operand, source, environment, sink);
                self.eval_unary(*operator, operand, node.span, sink)
            }
            AstKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.eval(left, source, environment, sink);
                let right = self.eval(right, source, environment, sink);
                self.eval_binary(*operator, left, right, node.span, sink)
            }
            AstKind::Assignment { target, value } => {
                let name = match &target.kind {
                    AstKind::Identifier => lexeme(source, target.span),
                    _ => {
                        sink.emit(Diagnostic::error("invalid assignment target").at(target.span));
                        return Value::Unit;
                    }
                };
                let value = self.eval(value, source, environment, sink);
                if !environment.assign(name, value.clone()) {
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
        operator: char,
        left: Value,
        right: Value,
        span: Span,
        sink: &mut DiagnosticSink,
    ) -> Value {
        match operator {
            '+' | '-' | '*' | '/' | '%' | '^' => {
                let (a, b) = match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => (a, b),
                    (l, r) => return binary_type_error(operator, &l, &r, span, sink),
                };
                match operator {
                    '+' => match a.checked_add(b) {
                        Some(sum) => Value::Integer(sum),
                        None => overflow(span, sink),
                    },
                    '-' => match a.checked_sub(b) {
                        Some(difference) => Value::Integer(difference),
                        None => overflow(span, sink),
                    },
                    '*' => match a.checked_mul(b) {
                        Some(product) => Value::Integer(product),
                        None => overflow(span, sink),
                    },
                    '/' if b == 0 => division_by_zero(span, sink),
                    '/' => match a.checked_div(b) {
                        Some(quotient) => Value::Integer(quotient),
                        None => overflow(span, sink),
                    },
                    '%' if b == 0 => division_by_zero(span, sink),
                    '%' => match a.checked_rem(b) {
                        Some(remainder) => Value::Integer(remainder),
                        None => overflow(span, sink),
                    },
                    '^' if b < 0 => {
                        sink.emit(
                            Diagnostic::error("negative exponents are not supported").at(span),
                        );
                        Value::Unit
                    }
                    '^' => match a.checked_pow(b as u32) {
                        Some(power) => Value::Integer(power),
                        None => overflow(span, sink),
                    },
                    _ => unreachable!("`operator` is restricted to arithmetic operators"),
                }
            }
            '<' | '>' => {
                let (a, b) = match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => (a, b),
                    (l, r) => return binary_type_error(operator, &l, &r, span, sink),
                };
                Value::Boolean(if operator == '<' { a < b } else { a > b })
            }
            '&' | '|' => {
                let (a, b) = match (left, right) {
                    (Value::Boolean(a), Value::Boolean(b)) => (a, b),
                    (l, r) => return binary_type_error(operator, &l, &r, span, sink),
                };
                Value::Boolean(if operator == '&' { a && b } else { a || b })
            }
            _ => {
                sink.emit(Diagnostic::error(format!("unknown operator `{operator}`")).at(span));
                Value::Unit
            }
        }
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// Extracts the source text covered by `span`.
fn lexeme(source: &SourceFile, span: Span) -> &str {
    &source.contents()[span.start..span.end]
}

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

fn binary_type_error(
    operator: char,
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

fn division_by_zero(span: Span, sink: &mut DiagnosticSink) -> Value {
    sink.emit(Diagnostic::error("division by zero").at(span));
    Value::Unit
}

fn overflow(span: Span, sink: &mut DiagnosticSink) -> Value {
    sink.emit(Diagnostic::error("integer overflow").at(span));
    Value::Unit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn eval(source_text: &str) -> (Value, DiagnosticSink) {
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
        assert_eq!(value, Value::Integer(42));
    }

    #[test]
    fn respects_operator_precedence() {
        let (value, sink) = eval("2 + 3 * 4;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(14));
    }

    #[test]
    fn exponentiation_binds_tighter_than_multiplication() {
        let (value, sink) = eval("2 ^ 3 * 2;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(16));
    }

    #[test]
    fn evaluates_bindings_and_references() {
        let (value, sink) = eval("let x = 5; x + 1;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(6));
    }

    #[test]
    fn assignment_updates_existing_bindings() {
        let (value, sink) = eval("let x = 5; x = 10; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(10));
    }

    #[test]
    fn blocks_introduce_new_scopes() {
        let (value, sink) = eval("let x = 5; { let x = 10; }; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(5));
    }

    #[test]
    fn comparisons_and_logic_produce_booleans() {
        let (value, sink) = eval("1 < 2;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Boolean(true));

        let (value, sink) = eval("!(1 < 2);");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Boolean(false));

        let (value, sink) = eval("1 < 2 & 2 < 3;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Boolean(true));
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
}
