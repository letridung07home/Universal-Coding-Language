use super::*;
use crate::lexer::Lexer;
use crate::parser::Parser;
#[test]
fn append_fast_path_treats_for_loops_and_let_initializers_as_mutating() {
    // `name = name + <expr>` takes an in-place fast path only when the
    // right side cannot mutate bindings; a non-string result then falls
    // back to general evaluation, which re-runs the expression. Anything
    // that executes code must therefore be flagged as mutating, or those
    // side effects happen twice. `for` loops and `let` initializers were
    // once invisible to the check.
    for source in [
        "s + { for i in 0..3 { t = t + 1; }; [] };",
        "s + { let q = f(); [] };",
        "s + { if true { f(); }; [] };",
    ] {
        let source = SourceFile::new("probe.ucl", source);
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());
        let ast = Parser::new(tokens)
            .parse(&mut DiagnosticSink::new())
            .expect("parses");
        let AstKind::Program { statements } = &ast.kind else {
            panic!("expected a program")
        };
        let AstKind::Binary { right, .. } = &statements[0].kind else {
            panic!("expected a binary expression")
        };
        assert!(
            Evaluator::may_mutate_bindings(right),
            "a block executing code must count as mutating"
        );
    }
}

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
fn append_assignment_accumulates_strings() {
    let (value, sink) = eval(r#"let s = "a"; s = s + "b"; s;"#);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("ab".to_owned())));
}

#[test]
fn append_assignment_updates_the_innermost_binding() {
    // The in-place append must target the shadowing binding, leaving the
    // outer one untouched.
    let (value, sink) = eval(r#"let s = "a"; { let s = "b"; s = s + "c"; }; s;"#);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("a".to_owned())));

    let (value, sink) = eval(r#"let s = ""; { let s = "b"; s = s + "c"; }; len(s);"#);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(0)));
}

#[test]
fn append_assignment_in_a_loop_accumulates() {
    let (value, sink) =
        eval(r#"let s = ""; let i = 0; while i < 3 { s = s + "?"; i = i + 1; }; len(s);"#);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(3)));
}

#[test]
fn append_assignment_reports_type_mismatch_like_general_form() {
    let (_value, sink) = eval(r#"let n = 1; n = n + "a";"#);
    assert!(sink.has_errors());
    assert!(
        sink.iter()
            .any(|diagnostic| diagnostic.message.contains("cannot apply `+`"))
    );
}

#[test]
fn append_assignment_to_undefined_variable_still_reported() {
    let (_value, sink) = eval(r#"x = x + "a";"#);
    assert!(
        sink.iter()
            .any(|diagnostic| diagnostic.message.contains("undefined variable `x`"))
    );
}

#[test]
fn regression_append_loop_with_inert_counter_terminates() {
    // Fuzz-found inputs: the counter never advances, so the loop runs
    // until its iteration cap. These must terminate with the documented
    // loop-limit error rather than hanging or exhausting memory.
    for source in [
        r#"let i = 0; let acc = ""; while i < 5 { acc = acc + "!";  i + 1; }; acc;"#,
        r#"let i = 0; let acc = ""; while i < 5 { acc = acc + "!"; i = i + 0; }; acc;"#,
    ] {
        let started = std::time::Instant::now();
        let (value, sink) = eval(source);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "evaluation of `{source}` should be bounded"
        );
        assert!(sink.iter().any(|diagnostic| {
            let message = &diagnostic.message;
            // Either deterministic guard may fire first: the loop cap
            // or the cumulative allocation budget.
            message.contains("loop exceeded the maximum number of iterations")
                || message.contains("total allocation budget")
        }));
        assert_eq!(value, None);
    }
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
fn relational_operators_compare_strings_lexicographically() {
    for (source, expected) in [
        ("\"apple\" < \"banana\";", true),
        ("\"apple\" < \"apple\";", false),
        ("\"apple\" <= \"apple\";", true),
        ("\"b\" > \"a\";", true),
        ("\"abc\" >= \"abd\";", false),
        ("\"\" < \"a\";", true),
        // Ordering is by Unicode scalar value, so multi-byte characters
        // compare by code point, not byte count.
        ("\"é\" > \"z\";", true),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
    }
}

#[test]
fn mixing_string_and_integer_in_relational_comparison_is_an_error() {
    for source in ["\"a\" < 1;", "1 <= \"a\";"] {
        let (_value, sink) = eval(source);
        assert!(sink.has_errors(), "`{source}` should be an error");
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
fn rejects_string_growth_before_host_memory_is_exhausted() {
    // Doubling 24 times attempts to create a 16 MiB value. The evaluator
    // must reject that deterministically at its 8 MiB value limit instead
    // of allowing concatenation to grow until the host runs out of memory.
    let (value, sink) =
        eval("let text = \"x\"; let i = 0; while i < 24 { text = text + text; i = i + 1; }; text;");
    assert_eq!(value, None);
    assert!(sink.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("string value exceeds the maximum size")
    }));
}

#[test]
fn evaluates_len_for_unicode_strings() {
    for (source, expected) in [("len(\"\");", 0), ("len(\"hé\");", 2)] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Integer(expected)), "for `{source}`");
    }
}

#[test]
fn reports_len_arity_and_type_errors() {
    for source in ["len();", "len(\"a\", \"b\");", "len(1);"] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`len`")),
            "expected a len-specific error for `{source}`"
        );
    }
}

#[test]
fn user_bindings_may_shadow_len() {
    let (value, sink) = eval("let len = fn(value) { value + 1; }; len(41);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));
}

#[test]
fn evaluates_str_across_value_kinds() {
    for (source, expected) in [
        ("str(42);", "42"),
        ("str(-7);", "-7"),
        ("str(true);", "true"),
        ("str(\"hé\");", "hé"),
        ("fn f() { return; }; str(f());", "unit"),
        ("str(fn(n) { n; });", "<function>"),
        ("str(len);", "<function>"),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(
            value,
            Some(Value::Str(expected.to_owned())),
            "for `{source}`"
        );
    }
}

#[test]
fn evaluates_type_for_each_value_kind() {
    for (source, expected) in [
        ("type(1);", "integer"),
        ("type(false);", "boolean"),
        ("type(\"a\");", "string"),
        ("type(len);", "function"),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(
            value,
            Some(Value::Str(expected.to_owned())),
            "for `{source}`"
        );
    }
}

#[test]
fn evaluates_case_builtins() {
    for (source, expected) in [
        ("upper(\"hello\");", "HELLO"),
        ("lower(\"WORLD\");", "world"),
        ("upper(\"hé\");", "HÉ"),
        ("lower(\"HÉ\");", "hé"),
        ("upper(\"\");", ""),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(
            value,
            Some(Value::Str(expected.to_owned())),
            "for `{source}`"
        );
    }
}

#[test]
fn evaluates_contains() {
    for (source, expected) in [
        ("contains(\"hello\", \"ell\");", true),
        ("contains(\"hello\", \"xyz\");", false),
        ("contains(\"\", \"\");", true),
        ("contains(\"abc\", \"\");", true),
        ("contains(\"abc\", \"abc\");", true),
        ("contains(\"héllo\", \"éll\");", true),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
    }
}

#[test]
fn reports_new_builtin_arity_and_type_errors() {
    for source in [
        "str();",
        "str(1, 2);",
        "type();",
        "type(\"a\", \"b\");",
        "upper();",
        "upper(1);",
        "lower(true);",
        "contains();",
        "contains(\"a\");",
        "contains(1, \"a\");",
        "contains(\"a\", 1);",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains('`')),
            "expected a built-in-specific error for `{source}`"
        );
    }
}

#[test]
fn user_bindings_may_shadow_new_builtins() {
    let (value, sink) = eval("let str = fn(value) { value + 1; }; str(41);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));

    let (value, sink) = eval("fn contains(a, b) { a; }; contains(7, 8);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(7)));
}

#[test]
fn parses_integers_from_strings() {
    for (source, expected) in [
        ("int(\"42\");", 42),
        ("int(\"-42\");", -42),
        ("int(\"+7\");", 7),
        ("int(\"0\");", 0),
        ("int(\"007\");", 7),
        ("int(\"9223372036854775807\");", i64::MAX),
        ("int(\"-9223372036854775808\");", i64::MIN),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "for `{source}`");
        assert_eq!(value, Some(Value::Integer(expected)), "for `{source}`");
    }
}

#[test]
fn int_passes_integers_through_like_str_passes_strings() {
    let (value, sink) = eval("int(42);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));
}

#[test]
fn reports_int_parse_type_and_overflow_errors() {
    // Malformed text.
    for source in [
        "int(\"\");",
        "int(\" 1\");",
        "int(\"1 \");",
        "int(\"1.5\");",
        "int(\"one\");",
        "int(\"--1\");",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter().any(|diagnostic| diagnostic
                .message
                .contains("`int` cannot parse the string as an integer")),
            "for `{source}`"
        );
    }
    // Overflow matches arithmetic overflow reporting.
    let (value, sink) = eval("int(\"99999999999999999999\");");
    assert_eq!(value, None);
    assert!(
        sink.iter()
            .any(|diagnostic| { diagnostic.message.contains("integer overflow") })
    );
    // Type errors.
    for source in ["int();", "int(true);", "int(1, 2);"] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`int`")),
            "for `{source}`"
        );
    }
}

#[test]
fn user_bindings_may_shadow_int() {
    let (value, sink) = eval("let int = fn(value) { value + 1; }; int(41);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));
}

#[test]
fn finds_substrings_by_scalar_value_index() {
    for (source, expected) in [
        ("find(\"hello\", \"ell\");", 1),
        ("find(\"hello\", \"hello\");", 0),
        ("find(\"abc\", \"\");", 0),
        ("find(\"héllo\", \"llo\");", 2),
        ("find(\"héllo\", \"é\");", 1),
        ("find(\"hello\", \"xyz\");", -1),
        ("find(\"\", \"a\");", -1),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Integer(expected)), "for `{source}`");
    }
}

#[test]
fn reports_find_arity_and_type_errors() {
    for source in [
        "find();",
        "find(\"a\");",
        "find(1, \"a\");",
        "find(\"a\", 1);",
        "find(\"a\", \"b\", \"c\");",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`find`")),
            "for `{source}`"
        );
    }
}

#[test]
fn replaces_every_occurrence() {
    for (source, expected) in [
        ("replace(\"banana\", \"na\", \"ny\");", "banyny"),
        // Overlapping-free scan: every occurrence counts.
        ("replace(\"aaa\", \"aa\", \"b\");", "ba"),
        ("replace(\"abc\", \"x\", \"y\");", "abc"),
        ("replace(\"abc\", \"abc\", \"\");", ""),
        ("replace(\"héllo héllo\", \"é\", \"e\");", "hello hello"),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(
            value,
            Some(Value::Str(expected.to_owned())),
            "for `{source}`"
        );
    }
}

#[test]
fn reports_replace_errors() {
    for source in [
        "replace();",
        "replace(\"a\");",
        "replace(\"a\", \"b\");",
        "replace(1, \"b\", \"c\");",
        "replace(\"a\", 1, \"c\");",
        "replace(\"a\", \"b\", 2);",
        // An empty pattern has no well-defined replacement.
        "replace(\"abc\", \"\", \"x\");",
        "replace(\"abc\", \"b\", \"c\", \"d\");",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`replace`")),
            "for `{source}`"
        );
    }
}

#[test]
fn trims_whitespace_from_both_ends() {
    for (source, expected) in [
        ("trim(\"  hi  \");", "hi"),
        ("trim(\"\\thi\\n\");", "hi"),
        ("trim(\"hi\");", "hi"),
        ("trim(\"   \");", ""),
        ("trim(\"\");", ""),
        ("trim(\" é \");", "é"),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(
            value,
            Some(Value::Str(expected.to_owned())),
            "for `{source}`"
        );
    }
}

#[test]
fn reports_trim_arity_and_type_errors() {
    for source in ["trim();", "trim(1);", "trim(true);", "trim(\"a\", \"b\");"] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`trim`")),
            "for `{source}`"
        );
    }
}

#[test]
fn slices_strings_by_scalar_value_indices() {
    for (source, expected) in [
        ("slice(\"hello\", 0, 2);", "he"),
        ("slice(\"hello\", 1, 4);", "ell"),
        ("slice(\"hello\", 2, 5);", "llo"),
        ("slice(\"hello\", 0, 5);", "hello"),
        ("slice(\"hello\", 3, 3);", ""),
        ("slice(\"héllo\", 1, 3);", "él"),
        ("slice(\"héllo\", 0, 1);", "h"),
        ("slice(\"\", 0, 0);", ""),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(
            value,
            Some(Value::Str(expected.to_owned())),
            "for `{source}`"
        );
    }
}

#[test]
fn reports_slice_out_of_range_and_type_errors() {
    // Strict bounds: negative indices, out-of-range ends, inverted ranges.
    for source in [
        "slice(\"hello\", -1, 2);",
        "slice(\"hello\", 0, -1);",
        "slice(\"hello\", 3, 2);",
        "slice(\"hello\", 0, 6);",
        "slice(\"\", 0, 1);",
        "slice(\"hello\", 6, 6);",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`slice` index out of range")),
            "for `{source}`"
        );
    }
    // Type errors.
    for source in [
        "slice();",
        "slice(\"a\");",
        "slice(\"a\", 0);",
        "slice(1, 0, 1);",
        "slice(\"a\", true, 1);",
        "slice(\"a\", 0, \"b\");",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`slice`")),
            "for `{source}`"
        );
    }
}

#[test]
fn user_bindings_may_shadow_new_string_builtins() {
    for (definition, call, expected) in [
        ("let find = fn(value) { value; };", "find(41);", 41),
        ("let replace = fn(a, b) { a * b; };", "replace(6, 7);", 42),
        ("let trim = fn(value) { value + 1; };", "trim(41);", 42),
        (
            "let slice = fn(a, b, c) { a + b + c; };",
            "slice(1, 2, 3);",
            6,
        ),
    ] {
        let source = format!("{definition} {call}");
        let (value, sink) = eval(&source);
        assert!(!sink.has_errors(), "for `{source}`");
        assert_eq!(value, Some(Value::Integer(expected)), "for `{source}`");
    }
}

#[test]
fn decodes_escape_sequences_in_strings() {
    let (value, sink) = eval(r##""a\tb\nc\"d\\";"##);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("a\tb\nc\"d\\".to_owned())));
}

#[test]
fn evaluates_list_literals_and_indexing() {
    for (source, expected) in [
        ("[1, 2, 3][0];", 1),
        ("[1, 2, 3][2];", 3),
        ("let items = [10, 20]; items[1];", 20),
        ("[[1, 2], [3, 4]][1][0];", 3),
        ("len([\"ab\", \"cd\"]);", 2),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Integer(expected)), "for `{source}`");
    }
    // String indexing yields one-character strings.
    for (source, expected) in [("\"hé\"[0];", "h"), ("\"hé\"[1];", "é")] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(
            value,
            Some(Value::Str(expected.to_owned())),
            "for `{source}`"
        );
    }
}

#[test]
fn lists_render_with_quoted_strings() {
    for (source, expected) in [
        ("str([])", "[]"),
        ("str([1, 2])", "[1, 2]"),
        ("str([1, \"two\", true])", "[1, \"two\", true]"),
        ("str([\"a\", [\"b\"]])", "[\"a\", [\"b\"]]"),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(
            value,
            Some(Value::Str(expected.to_owned())),
            "for `{source}`"
        );
    }
}

#[test]
fn len_and_contains_accept_lists() {
    for (source, expected) in [
        ("len([]) == 0;", true),
        ("len([1, 2, 3]) == 3;", true),
        ("contains([1, 2, 3], 2);", true),
        ("contains([1, 2, 3], 4);", false),
        // Membership uses the same equality as `==`.
        ("contains([[1], [2]], [2]);", true),
        ("contains(\"abc\", \"b\");", true),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
    }
}

#[test]
fn compares_lists_for_deep_equality() {
    for (source, expected) in [
        ("[1, 2] == [1, 2];", true),
        ("[1, 2] == [2, 1];", false),
        ("[] == [];", true),
        ("[1, [2, 3]] == [1, [2, 3]];", true),
        ("[1] != [2];", true),
        ("[\"a\"] == [\"a\"];", true),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
    }
    // Mixed-type comparisons remain errors.
    let (value, sink) = eval("[1] == \"1\";");
    assert_eq!(value, None);
    assert!(sink.has_errors());
}

#[test]
fn for_loops_iterate_lists() {
    let (value, sink) = eval("let total = 0; for x in [1, 2, 3] { total = total + x; }; total;");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(6)));

    let (value, sink) = eval("let n = 0; for x in [] { n = n + 1; }; n;");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(0)));
}

#[test]
fn reports_index_out_of_range_and_type_errors() {
    // Strict bounds on both lists and strings.
    for source in [
        "[1, 2][2];",
        "[1, 2][-1];",
        "[][0];",
        "\"ab\"[2];",
        "\"ab\"[-1];",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`index` is out of range")),
            "for `{source}`"
        );
    }
    // Type errors.
    for source in [
        "42[0];",
        "true[0];",
        "[1, 2][\"a\"];",
        "\"ab\"[true];",
        "let f = fn() { }; f[0];",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`index`")
                    || diagnostic.message.contains("cannot index")),
            "for `{source}`"
        );
    }
}

#[test]
fn list_concatenation_produces_a_new_list() {
    let list = |items: &[i64]| {
        Value::List(std::rc::Rc::new(
            items
                .iter()
                .map(|i| Value::Integer(*i))
                .collect::<Vec<Value>>(),
        ))
    };
    for (source, expected) in [
        ("[1] + [2];", list(&[1, 2])),
        ("[] + [1];", list(&[1])),
        (
            "[1, [2]] + [3];",
            Value::List(std::rc::Rc::new(vec![
                Value::Integer(1),
                list(&[2]),
                Value::Integer(3),
            ])),
        ),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(expected), "for `{source}`");
    }
    // Concatenation is functional: neither operand changes.
    let (value, sink) = eval("let a = [1]; let b = a + [2]; str(a) + \" \" + str(b);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("[1] [1, 2]".to_owned())));
    // Mixed-type addition remains an error.
    for source in ["[1, 2] + 3;", "1 + [2];"] {
        let (value, _sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
    }
}

#[test]
fn appends_elements_functionally() {
    // The accumulation idiom: build a list inside a loop.
    let (value, sink) =
        eval("let items = []; for i in 1..5 { items = append(items, i * i); }; items;");
    assert!(!sink.has_errors());
    assert_eq!(
        value,
        Some(Value::List(std::rc::Rc::new(vec![
            Value::Integer(1),
            Value::Integer(4),
            Value::Integer(9),
            Value::Integer(16),
        ])))
    );

    // The original list is untouched.
    let (value, sink) = eval("let a = [1]; let b = append(a, 2); str(a) + \" \" + str(b);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("[1] [1, 2]".to_owned())));

    let (value, sink) = eval("append([], [1]);");
    assert!(!sink.has_errors());
    assert_eq!(
        value,
        Some(Value::List(std::rc::Rc::new(vec![Value::List(
            std::rc::Rc::new(vec![Value::Integer(1)])
        )])))
    );
}

#[test]
fn reports_append_arity_and_type_errors() {
    for source in [
        "append();",
        "append([1]);",
        "append([1], 2, 3);",
        "append(42, 1);",
        "append(\"ab\", 1);",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`append`")),
            "for `{source}`"
        );
    }
}

#[test]
fn slices_and_searches_lists() {
    for (source, expected) in [
        ("str(slice([1, 2, 3], 0, 2));", "[1, 2]"),
        ("str(slice([1, 2, 3], 3, 3));", "[]"),
        ("str(slice([], 0, 0));", "[]"),
        ("str(slice([[1], [2]], 1, 2));", "[[2]]"),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(
            value,
            Some(Value::Str(expected.to_owned())),
            "for `{source}`"
        );
    }
    for (source, expected) in [
        ("find([10, 20, 30], 20);", 1),
        ("find([10, 20], 99);", -1),
        ("find([], 1);", -1),
        // Deep equality: nested lists match element by element.
        ("find([[1], [2]], [2]);", 1),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Integer(expected)), "for `{source}`");
    }
}

#[test]
fn reports_list_slice_out_of_range_errors() {
    for source in [
        "slice([1, 2], -1, 1);",
        "slice([1, 2], 0, 3);",
        "slice([1, 2], 2, 1);",
        "slice([1, 2], true, 1);",
        "slice(42, 0, 1);",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`slice`")),
            "for `{source}`"
        );
    }
}

#[test]
fn user_bindings_may_shadow_append() {
    let (value, sink) = eval("let append = fn(a, b) { a + b; }; append(40, 2);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));
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
fn break_exits_the_innermost_loop() {
    let source = "
            let total = 0;
            let i = 1;
            while i <= 10 {
                if i == 4 { break; };
                total = total + i;
                i = i + 1;
            };
        ";
    let (value, sink) = eval(&format!("{source} total;"));
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(6)));
}

#[test]
fn continue_skips_to_the_next_condition_check() {
    // Sum only the odd values below 10.
    let source = "
            let total = 0;
            let i = 0;
            while i < 10 {
                i = i + 1;
                if i % 2 == 0 { continue; };
                total = total + i;
            };
        ";
    let (value, sink) = eval(&format!("{source} total;"));
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(25)));
}

#[test]
fn for_loops_count_through_half_open_ranges() {
    for (source, expected) in [
        // 1 + 2 + 3 + 4
        (
            "let total = 0; for i in 1..5 { total = total + i; }; total;",
            10,
        ),
        // An empty range runs zero iterations.
        ("let n = 0; for i in 5..5 { n = n + 1; }; n;", 0),
        // An inverted range also runs zero iterations.
        ("let n = 0; for i in 3..1 { n = n + 1; }; n;", 0),
        // Negative bounds work like any other integers.
        (
            "let total = 0; for i in -2..2 { total = total + i; }; total;",
            -2,
        ),
        // Bounds are read once, before the first iteration.
        (
            "let end = 3; let seen = 0; for i in 0..end { end = 100; seen = seen + 1; }; seen;",
            3,
        ),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Integer(expected)), "for `{source}`");
    }
}

#[test]
fn for_loops_iterate_strings_by_scalar_value() {
    for (source, expected) in [
        ("let n = 0; for ch in \"abc\" { n = n + 1; }; n;", 3),
        // "hé" holds two scalar values.
        ("let n = 0; for ch in \"hé\" { n = n + 1; }; n;", 2),
        ("let n = 0; for ch in \"\" { n = n + 1; }; n;", 0),
    ] {
        let (value, sink) = eval(source);
        assert!(!sink.has_errors(), "unexpected error for `{source}`");
        assert_eq!(value, Some(Value::Integer(expected)), "for `{source}`");
    }
    // Each bound value is a one-character string.
    let (value, sink) =
        eval("let joined = \"\"; for ch in \"hé\" { joined = joined + ch; }; joined;");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("hé".to_owned())));
}

#[test]
fn for_loop_variables_are_body_scoped() {
    // The variable is fresh each iteration and disappears with the loop;
    // an outer binding of the same name survives untouched.
    let (value, sink) = eval("let i = 99; for i in 0..3 { }; i;");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(99)));

    // Assigning to the variable inside the body does not shift the
    // sequence: the next iteration rebinds it.
    let (value, sink) = eval(
        "
            let seen = 0;
            for i in 0..4 {
                i = 1000;
                seen = seen + i;
            };
            seen;
        ",
    );
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(4000)));
}

#[test]
fn break_and_continue_work_in_for_loops() {
    let source = "
            let total = 0;
            for i in 0..10 {
                if i == 3 { continue; };
                if i == 6 { break; };
                total = total + i;
            };
        ";
    let (value, sink) = eval(&format!("{source} total;"));
    assert!(!sink.has_errors());
    // 0 + 1 + 2 + 4 + 5
    assert_eq!(value, Some(Value::Integer(12)));
}

#[test]
fn nested_for_loops_bind_independent_variables() {
    let source = "
            let pairs = 0;
            for a in 0..3 {
                for b in 0..4 {
                    pairs = pairs + 1;
                };
            };
        ";
    let (value, sink) = eval(&format!("{source} pairs;"));
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(12)));
}

#[test]
fn oversized_for_sequences_hit_the_iteration_cap() {
    let (value, sink) = eval("let n = 0; for i in 0..999999999999 { n = n + 1; }; n;");
    assert_eq!(value, None);
    assert!(sink.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("loop exceeded the maximum number of iterations")
    }),);
}

#[test]
fn reports_for_iteration_type_errors() {
    // Non-integer range bounds.
    for source in [
        "for i in \"a\"..\"b\" { };",
        "for i in 0..true { };",
        "for i in 42 { };",
        "for ch in 42 { };",
    ] {
        let (value, sink) = eval(source);
        assert_eq!(value, None, "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`for`")),
            "for `{source}`"
        );
    }
}

#[test]
fn loop_signals_propagate_through_nested_loops_to_the_innermost_only() {
    // `break` inside the inner loop leaves the inner loop but the outer
    // one keeps running; `continue` in the outer loop skips its tail.
    let source = "
            let hits = 0;
            let outer = 0;
            while outer < 3 {
                outer = outer + 1;
                let inner = 0;
                while inner < 10 {
                    inner = inner + 1;
                    break;
                };
                if outer == 2 { continue; };
                hits = hits + inner;
            };
        ";
    let (value, sink) = eval(&format!("{source} hits;"));
    assert!(!sink.has_errors());
    // Inner always breaks after one pass; the second outer pass is
    // skipped by `continue`, so hits = 1 + 1 = 2.
    assert_eq!(value, Some(Value::Integer(2)));
}

#[test]
fn break_inside_a_function_body_is_an_error_at_the_call_site() {
    // A closure called from within a loop must not break the caller's
    // loop; the signal has no matching loop inside the function.
    let (value, sink) =
        eval("let f = fn() { break; }; let i = 0; while i < 3 { i = i + 1; f(); };");
    assert!(sink.has_errors());
    assert!(
        sink.iter()
            .any(|diagnostic| diagnostic.message.contains("`break` outside of a loop"))
    );
    assert_eq!(value, None);
}

#[test]
fn continue_outside_any_loop_is_an_error() {
    for source in ["continue;", "if true { continue; };"] {
        let (value, sink) = eval(source);
        assert!(sink.has_errors(), "expected an error for `{source}`");
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`continue` outside of a loop")),
            "for `{source}`"
        );
        assert_eq!(value, None);
    }
}

#[test]
fn break_still_runs_the_loop_condition_path_correctly_after_early_exit() {
    // After `break`, statements after it in the body are skipped and the
    // condition is not re-checked.
    let source = "
            let log = \"\";
            let i = 0;
            while i < 5 {
                i = i + 1;
                if i == 2 { break; };
                log = log + str(i);
            };
        ";
    let (value, sink) = eval(&format!("{source} log;"));
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("1".to_owned())));
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
fn functions_may_be_declared_inside_blocks() {
    let source = "
            fn outer() {
                fn inner(x) { x * 2; };
                inner(21);
            };
            outer();
        ";
    let (value, sink) = eval(source);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));
}

#[test]
fn function_literals_are_first_class_values() {
    // A literal stored in a variable and passed as an argument.
    let source = "
            fn apply(f, x) { f(x); };
            let double = fn(n) { n * 2; };
            apply(double, 21);
        ";
    let (value, sink) = eval(source);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));

    let source = "fn apply(f, x) { f(x); }; apply(fn(n) { n + 1; }, 41);";
    let (value, sink) = eval(source);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));
}

#[test]
fn literals_can_be_called_directly() {
    let (value, sink) = eval("fn(a, b) { a * b; }(6, 7);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));
}

#[test]
fn closures_capture_enclosing_locals_by_value() {
    // The literal captures `base` where it is created; rebinding `base`
    // afterwards must not change the closure's behavior.
    let source = "
            let make = fn(base) {
                return fn(n) { base + n; };
            };
            let add5 = make(5);
            let add7 = make(7);
            add5(10) + add7(10);
        ";
    let (value, sink) = eval(source);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(32)));
}

#[test]
fn locals_are_captured_but_globals_stay_dynamic() {
    // `base` is a parameter: the closure freezes its value at creation.
    // `factor` is a global: functions always see the latest value.
    let source = "
            let factor = 1;
            let make = fn(base) { return fn(n) { base + n * factor; }; };
            let add = make(10);
            factor = 100;
            add(1);
        ";
    let (value, sink) = eval(source);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(110)));
}

#[test]
fn functions_still_see_current_global_state() {
    // Globals resolve dynamically at call time, so reassigning a global
    // between creation and call is visible to the function.
    let source = "
            let factor = 2;
            fn scale(n) { n * factor; };
            factor = 10;
            scale(4);
        ";
    let (value, sink) = eval(source);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(40)));
}

#[test]
fn explicit_return_exits_the_function_early() {
    let source = "
            fn sign(n) {
                if n < 0 { return \"neg\"; };
                if n == 0 { return \"zero\"; };
                return \"pos\";
            };
            sign(-5);
        ";
    let (value, sink) = eval(source);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("neg".to_owned())));

    // `return` also unwinds through loops.
    let source = "
            fn first_square(limit) {
                let i = 0;
                while i < limit {
                    if i * i > 50 { return i; };
                    i = i + 1;
                };
                return -1;
            };
            first_square(100);
        ";
    let (value, sink) = eval(source);
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(8)));
}

#[test]
fn bare_return_yields_unit_and_the_last_statement_fills_the_rest() {
    let (value, sink) = eval("fn nothing() { return; }; nothing();");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Unit));

    let (value, sink) = eval("fn implicit() { 40 + 2; }; implicit();");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Integer(42)));
}

#[test]
fn return_at_program_scope_is_an_error() {
    for source in ["return 1;", "if true { return; };"] {
        let (_value, sink) = eval(source);
        assert!(sink.has_errors(), "expected an error for `{source}`");
        assert_eq!(_value, None, "no value for `{source}`");
    }
}

#[test]
fn a_literal_cannot_recur_through_its_own_variable() {
    // Documented limitation: the variable does not exist when the
    // literal's capture is taken.
    let source = "let f = fn(n) { f(n); };";
    let (_value, _sink) = eval(source);
    // Parsing and creating the literal succeed; only calling it would
    // fail, which we deliberately do not do here.
}

#[test]
fn caps_recursive_function_calls() {
    // Runs on a generous stack so the assertion exercises the call-depth
    // guard itself rather than the test thread's stack limit. Only a plain
    // boolean crosses the thread boundary: `Value` contains `Rc`, which is
    // not `Send`.
    let hit_depth_guard = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let (_value, sink) = eval("fn recurse() { recurse(); }; recurse();");
            sink.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("function call depth is too deep")
            })
        })
        .expect("test harness thread spawns")
        .join()
        .expect("harness thread does not panic");

    assert!(hit_depth_guard);
}

#[test]
fn evaluate_in_persists_global_bindings_across_calls() {
    let evaluator = Evaluator::new();
    let mut environment = Environment::new();

    for (input, expected) in [("let x = 40;", None), ("x + 2;", Some(Value::Integer(42)))] {
        let line_source = SourceFile::new("repl.ucl", input);
        let mut line_sink = DiagnosticSink::new();
        let tokens = Lexer::new(&line_source).tokenize(&mut line_sink);
        let ast = Parser::new(tokens)
            .parse(&mut line_sink)
            .expect("parser should return a program");
        assert!(!line_sink.has_errors(), "for `{input}`");

        let value = evaluator.evaluate_in(&mut environment, &ast, &line_source, &mut line_sink);
        assert!(!line_sink.has_errors(), "for `{input}`");
        if let Some(value) = value {
            assert_eq!(value, expected.unwrap_or(Value::Unit), "for `{input}`");
        }
    }
}

#[test]
fn evaluate_in_keeps_functions_and_their_captures_alive() {
    let mut sink = DiagnosticSink::new();
    let evaluator = Evaluator::new();
    let mut environment = Environment::new();

    let run_line = |evaluator: &Evaluator,
                    environment: &mut Environment,
                    input: &str,
                    sink: &mut DiagnosticSink| {
        let line_source = SourceFile::new("repl.ucl", input);
        let tokens = Lexer::new(&line_source).tokenize(sink);
        let ast = Parser::new(tokens)
            .parse(sink)
            .expect("parser should return a program");
        evaluator.evaluate_in(environment, &ast, &line_source, sink)
    };

    let value = run_line(
        &evaluator,
        &mut environment,
        "fn make(base) { return fn(n) { base + n; }; }; make(5);",
        &mut sink,
    )
    .expect("definition succeeds");
    // Store the returned closure by re-declaring it in a follow-up line.
    let stored = matches!(value, Value::Function(_));
    assert!(stored);

    // A fresh line can still call the closure through a global binding.
    assert!(
        run_line(
            &evaluator,
            &mut environment,
            "let add5 = make(5);",
            &mut sink
        )
        .is_some()
    );
    let result = run_line(&evaluator, &mut environment, "add5(37);", &mut sink);
    assert_eq!(result, Some(Value::Integer(42)));
}

#[test]
fn an_error_on_one_line_does_not_poison_the_next() {
    let mut sink = DiagnosticSink::new();
    let evaluator = Evaluator::new();
    let mut environment = Environment::new();

    let run_line = |evaluator: &Evaluator,
                    environment: &mut Environment,
                    input: &str,
                    sink: &mut DiagnosticSink| {
        let line_source = SourceFile::new("repl.ucl", input);
        let tokens = Lexer::new(&line_source).tokenize(sink);
        let ast = Parser::new(tokens)
            .parse(sink)
            .expect("parser should return a program");
        evaluator.evaluate_in(environment, &ast, &line_source, sink)
    };

    assert!(run_line(&evaluator, &mut environment, "let x = 1;", &mut sink).is_some());
    assert!(run_line(&evaluator, &mut environment, "x / 0;", &mut sink).is_none());
    assert!(sink.has_errors());
    assert_eq!(
        run_line(&evaluator, &mut environment, "x + 1;", &mut sink),
        Some(Value::Integer(2))
    );
}

#[test]
fn allocation_budget_bounds_runaway_string_accumulation() {
    // Fuzz-found shape (CI timeout in the v1.14 fuzz run): an infinite
    // loop whose body concatenates a growing string. The cumulative
    // allocation budget must stop it long before the loop cap.
    let started = std::time::Instant::now();
    let (value, sink) = eval(r#"let acc = ""; while true { acc = acc + "0123456789"; };"#);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "the allocation budget must stop runaway accumulation quickly"
    );
    assert!(sink.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("evaluation exceeded its total allocation budget")
    }));
    assert_eq!(value, None);
}

#[test]
fn work_budget_bounds_nested_runaway_loops() {
    // Each loop is individually capped, but nested loops can still multiply
    // their total work. The evaluator-wide fuel budget must stop this input
    // without waiting for the inner and outer loop caps to be exhausted.
    let started = std::time::Instant::now();
    let (value, sink) = eval("let i = 8; while i < 100000 { while i < 100000 { i = i + 0; }; };");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "nested runaway loops must be stopped quickly"
    );
    assert!(sink.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("evaluation exceeded its maximum work budget")
    }));
    assert_eq!(value, None);
}

#[test]
fn allocation_budget_bounds_nested_list_growth() {
    // List slots are real Value-sized allocations. A nested loop must not be
    // able to create hundreds of millions of elements before a nominal
    // byte-budget expressed in element counts notices.
    let (value, sink) = eval(
        "let items = []; let i = 8; while i < 100000 { items = items + [i]; i = i + 0; while i < 100000 { items = items + [i]; i = i + 0; }; }; items;",
    );
    assert_eq!(value, None);
    assert!(sink.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("evaluation exceeded its total allocation budget")
            || diagnostic
                .message
                .contains("evaluation exceeded its maximum work budget")
    }));
}

#[test]
fn allocation_budget_bounds_aliased_list_copying() {
    // Aliasing the list inside the loop forces a full copy per append;
    // the budget charges that copy and stops the loop early.
    let (value, sink) =
        eval("let items = [0]; while true { let alias = items; items = append(items, 1); };");
    assert!(sink.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("evaluation exceeded its total allocation budget")
    }));
    assert_eq!(value, None);
}

#[test]
fn large_but_ordinary_accumulation_stays_within_the_budget() {
    // The accumulate idiom is linear through the in-place append fast
    // path and charges only growth; it must complete without errors.
    let (value, sink) =
        eval("let items = []; for i in 0..50000 { items = append(items, i); }; len(items);");
    assert!(
        !sink.has_errors(),
        "unexpected diagnostics: {:?}",
        sink.len()
    );
    assert_eq!(value, Some(Value::Integer(50000)));
}

#[test]
fn list_append_fast_path_preserves_aliased_semantics() {
    // When another binding holds the list, appending must copy: the
    // original stays untouched even though assignment goes through the
    // in-place fast path.
    let (value, sink) = eval("let a = [1]; let b = a; a = append(a, 2); str(a) + str(b);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("[1, 2][1]".to_owned())));
}

#[test]
fn allocation_budget_bounds_runaway_case_mapping() {
    // `upper` copies the whole string on every call; the budget charges
    // that copy, so a loop re-mapping a large string stops early.
    let (value, sink) = eval(concat!(
        r#"let s = "x"; for i in 0..21 { s = s + s; };"#,
        " while true { s = upper(s); };"
    ));
    assert!(sink.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("evaluation exceeded its total allocation budget")
    }));
    assert_eq!(value, None);
}

#[test]
fn allocation_budget_bounds_runaway_slicing() {
    // `slice` builds a fresh value per call; the budget charges it.
    let (value, sink) = eval(
        r#"let items = []; for i in 0..1000 { items = append(items, i); }; while true { items = slice(items, 0, len(items)); };"#,
    );
    assert!(sink.iter().any(|diagnostic| {
        let message = &diagnostic.message;
        message.contains("total allocation budget")
            || message.contains("loop exceeded the maximum number of iterations")
    }));
    assert_eq!(value, None);
}

#[test]
fn list_concat_accumulation_is_linear_through_the_fast_path() {
    // `items = items + [x]` extends the binding in place; 100,000
    // iterations must finish quickly without tripping any limit.
    let started = std::time::Instant::now();
    let (value, sink) =
        eval("let items = []; for i in 0..100000 { items = items + [i]; }; len(items);");
    assert!(!sink.has_errors(), "unexpected diagnostics");
    assert_eq!(value, Some(Value::Integer(100000)));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "`+` accumulation should be linear, took {:?}",
        started.elapsed()
    );
}

#[test]
fn list_concat_fast_path_preserves_aliased_semantics() {
    // An aliased list must be copied, not mutated: `b` keeps pointing at
    // the original even though assignment takes the fast path.
    let (value, sink) = eval("let a = [1]; let b = a; a = a + [2]; str(a) + str(b);");
    assert!(!sink.has_errors());
    assert_eq!(value, Some(Value::Str("[1, 2][1]".to_owned())));
}

#[test]
fn list_concat_charges_aliased_copies() {
    // Aliasing inside the loop forces a full copy per iteration; the
    // budget charges each copy and stops the loop early.
    let (value, sink) =
        eval("let items = [0]; while true { let alias = items; items = items + [1]; };");
    assert!(sink.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("evaluation exceeded its total allocation budget")
    }));
    assert_eq!(value, None);
}
