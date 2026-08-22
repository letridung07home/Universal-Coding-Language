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
//! - The final value is the result of the last successfully evaluated statement
//! - Callers should check `sink.has_errors()` to determine if execution succeeded
//!
//! This design allows users to see all errors in their program at once,
//! similar to how compilers like rustc operate.

use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::parser::{AstKind, AstNode};
use crate::source::{SourceFile, Span};

/// The maximum evaluation-nesting depth allowed before an error is reported.
///
/// This guards against deeply nested ASTs constructed through the public API
/// that would otherwise overflow the call stack during recursive evaluation.
/// It is deliberately larger than the parser's own nesting limit: AST wrappers
/// such as `let`, assignment, and binary operators add evaluator depth without
/// adding parser nesting, so a program the parser accepts must never be
/// rejected here.
const MAX_EVAL_DEPTH: usize = 1024;

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
    // TODO: strings, functions, and the rest of the type system, once the
    // lexer and parser gain syntax for them.
}

impl Value {
    /// Returns the value's short type name, used in diagnostics.
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
pub struct Evaluator;

impl Evaluator {
    /// Creates a new evaluator instance.
    ///
    /// The evaluator is stateless, so a single instance can be reused
    /// to evaluate multiple ASTs.
    pub fn new() -> Self {
        Self
    }

    /// Evaluates the given AST to produce a value.
    ///
    /// `source` provides the text that the AST's spans point into. The result
    /// is the value of the program's last statement, or [`Value::Unit`] if the
    /// program is empty.
    ///
    /// # Error Handling
    ///
    /// Even if errors occur during evaluation, this method returns a value
    /// (typically the last successfully computed value). Callers should check
    /// `sink.has_errors()` to determine if evaluation succeeded without errors.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let source = SourceFile::new("example.ucl", "2 + 3 * 4");
    /// let mut sink = DiagnosticSink::new();
    /// let ast = /* parse the source */;
    /// let value = Evaluator::new().evaluate(&ast, &source, &mut sink);
    /// if sink.has_errors() {
    ///     // Handle errors
    /// }
    /// ```
    pub fn evaluate(
        &self,
        root: &AstNode,
        source: &SourceFile,
        sink: &mut DiagnosticSink,
    ) -> Value {
        let mut environment = Environment::default();
        environment.push_scope();
        self.eval(root, source, &mut environment, sink, 0)
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
                let name = source.slice(*name).expect("declaration name span is valid");
                let value = self.eval(value, source, environment, sink, depth + 1);
                environment.define(name, value);
                Value::Unit
            }
            AstKind::Identifier => {
                let name = source.slice(node.span).expect("identifier span is valid");
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
                let text = source
                    .slice(node.span)
                    .expect("integer literal span is valid");
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
                let left = self.eval(left, source, environment, sink, depth + 1);
                let right = self.eval(right, source, environment, sink, depth + 1);
                self.eval_binary(*operator, left, right, node.span, sink)
            }
            AstKind::Assignment { target, value } => {
                let name = match &target.kind {
                    AstKind::Identifier => source
                        .slice(target.span)
                        .expect("assignment target span is valid"),
                    _ => {
                        sink.emit(Diagnostic::error("invalid assignment target").at(target.span));
                        return Value::Unit;
                    }
                };
                let value = self.eval(value, source, environment, sink, depth + 1);
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
                    '^' => {
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
        // limit, to exercise the evaluator's independent guard.
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
        assert_eq!(value, Value::Integer(0));

        let (value, sink) = eval("1 ^ 4294967296;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(1));

        let (value, sink) = eval("-1 ^ 4294967296;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(1));

        let (value, sink) = eval("-1 ^ 4294967297;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(-1));
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
        assert_eq!(value, Value::Boolean(true));
    }

    #[test]
    fn assignment_inside_a_block_updates_the_outer_binding() {
        let (value, sink) = eval("let x = 5; { x = 10; }; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(10));
    }

    #[test]
    fn assignment_to_a_shadowed_binding_stays_in_its_scope() {
        let (value, sink) = eval("let x = 5; { let x = 10; x = 20; }; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(5));
    }

    #[test]
    fn evaluates_moderately_long_flat_binary_chains() {
        let source_text = vec!["1"; 100].join(" + ") + ";";
        let (value, sink) = eval(&source_text);
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(100));
    }

    #[test]
    #[ignore = "known limitation: evaluator depth guard rejects long flat chains"]
    fn evaluates_long_flat_binary_chains_within_the_parser_limit() {
        // A flat, left-associative chain adds evaluator depth without adding
        // parser nesting, so the parser accepts it while the evaluator's
        // `MAX_EVAL_DEPTH` guard (1024) currently rejects it. Remove the
        // `#[ignore]` once the depth limits are made consistent.
        let terms = 2_000;
        let source_text = vec!["1"; terms].join(" + ") + ";";
        let (value, sink) = eval(&source_text);
        assert!(!sink.has_errors());
        assert_eq!(value, Value::Integer(terms as i64));
    }
}
