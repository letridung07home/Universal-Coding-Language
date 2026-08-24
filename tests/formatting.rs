//! Formatter guarantees: idempotency, semantics preservation, and total
//! behavior (no panics) for any input.
//!
//! The formatter promises three things, checked here against a corpus of
//! representative programs and pseudo-random inputs with a fixed seed:
//!
//! - formatting is idempotent: `fmt(fmt(x))` equals `fmt(x)`;
//! - formatting preserves semantics: a clean program and its formatted
//!   form evaluate to the same value;
//! - formatting never panics, even on input that cannot be formatted.

use std::panic::{AssertUnwindSafe, catch_unwind};

use ucl::{DiagnosticSink, Environment, Evaluator, Lexer, Parser, SourceFile, Value};

/// A deterministic splitmix64 generator (same design as property.rs).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

/// Characters likely to exercise interesting formatter paths.
const ALPHABET: &str = "abcxyz01+-*/%^<>&|!=;,(){}[] \t\n\"\\//letifnrfswuh";

fn random_string(rng: &mut Rng) -> String {
    let alphabet: Vec<char> = ALPHABET.chars().collect();
    let length = rng.below(80) as usize;
    (0..length)
        .map(|_| alphabet[rng.below(alphabet.len() as u64) as usize])
        .collect()
}

/// Programs covering every construct and the comment-placement edge cases.
const CORPUS: &[&str] = &[
    "",
    "\n\n",
    "let x = 1;",
    "let x=1;let y=x+2*3; y;",
    "if x > 1 { y = 2; } else { y = 3; }",
    "if a { 1; } else if b { 2; } else if c { 3; } else { 4; };",
    "while i < 10 { i = i + 1; if done { break; }; continue; };",
    "for i in 0..10 { total = total + i; };",
    "for item in items { use_item(item); };",
    "fn add(a, b) { return a + b; }; add(1, 2);",
    "let f = fn(x) { x * 2; }; f(21);",
    "fn outer() { fn inner() { return 1; }; inner(); };",
    "let xs = [1, [2, 3], \"four\", true]; xs[1][0];",
    "let long = [\n    111111,\n    222222,\n    333333,\n];\nlong[0];",
    "let empty = []; let nested = [[]]; len(empty);",
    "use \"math.ucl\" as m;\nuse \"util.ucl\";\nm.double(4);",
    "let s = \"tab\\t newline\\n quote\\\" back\\\\\";\ns;",
    "return_statement_free(); fn r() { return; }; r();",
    "// leading comment\nlet x = 1; // trailing\n// between\nlet y = 2; // end",
    "/* header\n * across lines\n */\nlet x = /* inline */ 5;",
    "fn f() {\n    // inside body\n    return 1; // after return\n    // before close\n};",
    "let a = 1;\n\n\n\nlet b = 2;\n\n\nlet c = 3;",
    "let deep = [[[[1]]]]; deep[0][0][0][0];",
    "f(g(h(1)), [2, 3], -x % 4);",
    "{ let scoped = 1; scoped; };",
    "while false { }; 1;",
];

/// Formats `source`, or returns `None` when it has lexical or syntax errors.
fn try_format(source_text: &str) -> Option<String> {
    ucl::fmt::format_source("format.ucl", source_text).ok()
}

/// Evaluates a program and reports whether it ran clean, plus its value.
fn evaluate(source_text: &str) -> Result<Option<Value>, ()> {
    let source = SourceFile::new("format.ucl", source_text);
    let mut sink = DiagnosticSink::new();
    let tokens = Lexer::new(&source).tokenize(&mut sink);
    if sink.has_errors() {
        return Err(());
    }
    let Some(ast) = Parser::new(tokens).parse(&mut sink) else {
        return Err(());
    };
    if sink.has_errors() {
        return Err(());
    }
    let mut environment = Environment::new();
    let value = Evaluator::new().evaluate_in(&mut environment, &ast, &source, &mut sink);
    if sink.has_errors() {
        return Err(());
    }
    Ok(value)
}

#[test]
fn corpus_formats_idempotently() {
    for source in CORPUS {
        let Some(once) = try_format(source) else {
            panic!("corpus member should format: {source:?}");
        };
        let Some(twice) = try_format(&once) else {
            panic!("formatted output should re-parse: {once:?}");
        };
        assert_eq!(twice, once, "not idempotent for {source:?}");
    }
}

#[test]
fn corpus_preserves_semantics() {
    for source in CORPUS {
        let Some(formatted) = try_format(source) else {
            panic!("corpus member should format: {source:?}");
        };
        match (evaluate(source), evaluate(&formatted)) {
            (Err(()), Err(())) => {}
            (Ok(a), Ok(b)) => assert_eq!(a, b, "semantics changed for {source:?} -> {formatted:?}"),
            (a, b) => {
                panic!("error status changed for {source:?}: original {a:?}, formatted {b:?}")
            }
        }
    }
}

#[test]
fn formatting_never_panics_on_arbitrary_input() {
    let mut rng = Rng(0xF0_471D_5EED);
    for input in (0..5_000).map(|_| random_string(&mut rng)) {
        let result = catch_unwind(AssertUnwindSafe(|| {
            if let Some(once) = try_format(&input)
                && let Some(twice) = try_format(&once)
            {
                assert_eq!(twice, once, "not idempotent for {input:?}");
            }
        }));
        assert!(result.is_ok(), "formatter panicked on input {input:?}");
    }
}

#[test]
fn diagnostics_are_reported_not_swallowed() {
    let broken = [
        "let = ;",
        "\"unterminated",
        "/* unterminated",
        "fn ( {",
        "1 + ",
    ];
    for source in broken {
        assert!(
            ucl::fmt::format_source("broken.ucl", source).is_err(),
            "expected diagnostics for {source:?}"
        );
    }
}

#[test]
fn crlf_and_final_newline_are_normalized() {
    let formatted = try_format("let a = 1;\r\nlet b = 2;").expect("formats");
    assert_eq!(formatted, "let a = 1;\nlet b = 2;\n");
}
