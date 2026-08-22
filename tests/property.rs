//! Property-style tests for the UCL pipeline.
//!
//! These tests use a small, deterministic pseudo-random number generator (no
//! external dependencies) to check invariants that must hold for *any* input:
//!
//! - the lexer → parser → evaluator pipeline never panics;
//! - the lexer always emits a trailing `Eof` token and only valid spans;
//! - integer arithmetic agrees with Rust's own checked arithmetic;
//! - operator precedence agrees with the language specification.
//!
//! Every run uses a fixed seed, so a failure can be reproduced exactly by
//! re-running the test.

use std::panic::{AssertUnwindSafe, catch_unwind};

use ucl::{DiagnosticSink, Evaluator, Lexer, Parser, SourceFile, TokenKind, Value};

/// A small, deterministic splitmix64 generator.
///
/// Property tests must be reproducible: the same seed always produces the
/// same sequence, so a failing input can be re-derived and minimized.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a value in `0..bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }

    /// Returns a value in the inclusive range `low..=high`.
    ///
    /// The computation is performed in `i128` so the full `i64` range can be
    /// used without overflowing the intermediate `high - low`.
    fn range(&mut self, low: i64, high: i64) -> i64 {
        let span = high as i128 - low as i128 + 1;
        (low as i128 + (self.next_u64() as i128 % span)) as i64
    }
}

/// Characters likely to exercise interesting lexer and parser paths.
const ALPHABET: &str = "abcxyzlet012379+-*/%^<>&|!=;(){} \t\néπ🙂";

/// Generates a random string up to 64 characters long.
fn random_string(rng: &mut Rng) -> String {
    let alphabet: Vec<char> = ALPHABET.chars().collect();
    let length = rng.below(65) as usize;
    let alphabet_len = alphabet.len() as u64;
    (0..length)
        .map(|_| alphabet[rng.below(alphabet_len) as usize])
        .collect()
}

/// Runs the full pipeline (lex → parse → evaluate) over `input`.
///
/// This must never panic for any input; a panic here is exactly the bug these
/// property tests are looking for.
fn run_pipeline(input: &str) {
    let source = SourceFile::new("property.ucl", input);
    let mut sink = DiagnosticSink::new();
    let tokens = Lexer::new(&source).tokenize(&mut sink);
    let ast = Parser::new(tokens).parse(&mut sink);
    if let Some(ast) = ast {
        let _ = Evaluator::new().evaluate(&ast, &source, &mut sink);
    }
}

/// Evaluates a single UCL expression and returns its value and error status.
fn eval_expression(source_text: &str) -> (Value, bool) {
    let source = SourceFile::new("property.ucl", source_text);
    let mut sink = DiagnosticSink::new();
    let tokens = Lexer::new(&source).tokenize(&mut sink);
    let ast = Parser::new(tokens).parse(&mut sink);
    let value = match ast {
        Some(ast) => Evaluator::new()
            .evaluate(&ast, &source, &mut sink)
            .unwrap_or(Value::Unit),
        None => Value::Unit,
    };
    (value, sink.has_errors())
}

/// Applies `operator` to `a` and `b` with the same checked semantics as the
/// evaluator, returning the result or an error tag.
fn checked_apply(operator: char, a: i64, b: i64) -> Result<i64, &'static str> {
    match operator {
        '+' => a.checked_add(b).ok_or("overflow"),
        '-' => a.checked_sub(b).ok_or("overflow"),
        '*' => a.checked_mul(b).ok_or("overflow"),
        '/' => a.checked_div(b).ok_or("division by zero"),
        '%' => a.checked_rem(b).ok_or("division by zero"),
        '^' => {
            let exponent = u32::try_from(b).map_err(|_| "exponent too large")?;
            a.checked_pow(exponent).ok_or("overflow")
        }
        _ => unreachable!("only arithmetic operators are generated"),
    }
}

/// Precedence levels matching `src/parser.rs` (higher binds tighter).
fn precedence(operator: char) -> u8 {
    match operator {
        '+' | '-' => 4,
        '*' | '/' => 5,
        '^' => 6,
        _ => unreachable!("only arithmetic operators are generated"),
    }
}

/// Evaluates `a op1 b op2 c` with the operator precedence defined by the
/// language specification (left-associative; `^` > `*`/`/` > `+`/`-`).
fn eval_precedence(a: i64, op1: char, b: i64, op2: char, c: i64) -> Option<i64> {
    if precedence(op1) >= precedence(op2) {
        let intermediate = checked_apply(op1, a, b).ok()?;
        checked_apply(op2, intermediate, c).ok()
    } else {
        let intermediate = checked_apply(op2, b, c).ok()?;
        checked_apply(op1, a, intermediate).ok()
    }
}

#[test]
fn pipeline_never_panics_on_arbitrary_input() {
    let mut rng = Rng::new(0x5EED_1234_5678_9ABC);
    for input in (0..5_000).map(|_| random_string(&mut rng)) {
        let result = catch_unwind(AssertUnwindSafe(|| run_pipeline(&input)));
        assert!(result.is_ok(), "pipeline panicked on input {input:?}");
    }
}

#[test]
fn lexer_spans_are_always_valid() {
    let mut rng = Rng::new(0x0BAD_5EED_0BAD_5EED);
    for input in (0..5_000).map(|_| random_string(&mut rng)) {
        let source = SourceFile::new("property.ucl", input.as_str());
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        let last = tokens.last().map(|token| token.kind);
        assert_eq!(
            last,
            Some(TokenKind::Eof),
            "missing trailing Eof for {input:?}"
        );
        for token in &tokens {
            let span = token.span;
            assert!(span.start <= span.end, "inverted span for {input:?}");
            assert!(
                span.end <= input.len(),
                "span past end of input for {input:?}"
            );
            assert!(
                input.is_char_boundary(span.start) && input.is_char_boundary(span.end),
                "span is not on char boundaries for {input:?}"
            );
        }
    }
}

#[test]
fn integer_arithmetic_matches_rust_checked_semantics() {
    let mut rng = Rng::new(0xACED_0001_ACED_0001);
    let operators = ['+', '-', '*', '/', '%', '^'];

    for _ in 0..2_000 {
        let operator = operators[rng.below(operators.len() as u64) as usize];
        // Operands cover the full signed range except `i64::MIN`, which has no
        // literal representation (see the language specification).
        let a = rng.range(-i64::MAX, i64::MAX);
        let mut b = rng.range(-i64::MAX, i64::MAX);

        // Division and remainder by zero are errors, not values, so pick a
        // non-zero divisor to compare against Rust's checked arithmetic.
        if matches!(operator, '/' | '%') && b == 0 {
            b = 1;
        }
        // The exponent must be non-negative and small enough to stay within
        // the evaluator's plain `checked_pow` path (see `src/evaluator.rs`).
        if operator == '^' {
            b = rng.range(0, 63);
        }

        let source_text = format!("{a} {operator} {b};");
        let (value, had_errors) = eval_expression(&source_text);

        match checked_apply(operator, a, b) {
            Ok(expected) => {
                assert!(!had_errors, "unexpected error for `{source_text}`");
                assert_eq!(
                    value,
                    Value::Integer(expected),
                    "mismatch for `{source_text}`"
                );
            }
            Err(_) => {
                assert!(had_errors, "expected an error for `{source_text}`");
            }
        }
    }
}

#[test]
fn operator_precedence_matches_the_specification() {
    let mut rng = Rng::new(0xDEAD_BEEF_DEAD_BEEF);
    let operators = ['+', '-', '*', '/', '^'];

    for _ in 0..2_000 {
        let op1 = operators[rng.below(operators.len() as u64) as usize];
        let op2 = operators[rng.below(operators.len() as u64) as usize];

        let a = rng.range(0, 9);
        let mut b = rng.range(0, 9);
        let mut c = rng.range(0, 9);

        // Avoid division by zero; other operands may be zero freely.
        if op1 == '/' && b == 0 {
            b = 1;
        }
        if op2 == '/' && c == 0 {
            c = 1;
        }

        let source_text = format!("{a} {op1} {b} {op2} {c};");
        let (value, had_errors) = eval_expression(&source_text);

        match eval_precedence(a, op1, b, op2, c) {
            Some(expected) => {
                assert!(!had_errors, "unexpected error for `{source_text}`");
                assert_eq!(
                    value,
                    Value::Integer(expected),
                    "mismatch for `{source_text}`"
                );
            }
            None => {
                assert!(had_errors, "expected an error for `{source_text}`");
            }
        }
    }
}
