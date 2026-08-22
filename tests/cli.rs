//! Integration tests that exercise the `ucl` command-line binary.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter for unique temporary file names.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `source` to a temporary file, evaluates it with the `ucl` binary,
/// and returns its stdout, stderr, and exit status.
fn run(source: &str) -> (String, String, bool) {
    let path = temp_path();
    fs::write(&path, source).expect("write source file");

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg(&path)
        .output()
        .expect("run the ucl binary");

    let _ = fs::remove_file(&path);

    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

/// Returns a unique temporary file path in the system temp directory.
fn temp_path() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ucl-cli-{}-{id}.ucl", std::process::id()))
}

/// Writes `source` to a temporary file, evaluates it, and returns its stdout,
/// stderr, and exit code.
fn run_file_with_code(source: &str) -> (String, String, i32) {
    let path = temp_path();
    fs::write(&path, source).expect("write source file");

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg(&path)
        .output()
        .expect("run the ucl binary");

    let _ = fs::remove_file(&path);

    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().expect("the process exited normally"),
    )
}

/// Runs the `ucl` binary with the given arguments and returns its stdout,
/// stderr, and exit code.
fn run_args(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .args(args)
        .output()
        .expect("run the ucl binary");

    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().expect("the process exited normally"),
    )
}

#[test]
fn evaluates_a_source_file_and_prints_the_result() {
    let (stdout, stderr, success) = run("2 + 3 * 4;");
    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "14\n");
    assert!(stderr.is_empty());
}

#[test]
fn prints_nothing_for_unit_results() {
    let (stdout, stderr, success) = run("let x = 5;");
    assert!(success, "stderr: {stderr}");
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn prints_string_results_as_their_contents() {
    let (stdout, stderr, success) = run("\"hello \" + \"world\";");
    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "hello world\n");
    assert!(stderr.is_empty());
}

#[test]
fn evaluates_if_else_and_while_end_to_end() {
    let source = "
        let n = 1;
        while n < 100 {
            n = n * 2;
        };
        if n == 128 { \"one twenty-eight\"; } else { \"unexpected\"; };
    ";
    let (stdout, stderr, success) = run(source);
    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "one twenty-eight\n");
    assert!(stderr.is_empty());
}

#[test]
fn evaluates_function_calls_end_to_end() {
    let source = "fn triple(value) { value * 3; }; triple(14);";
    let (stdout, stderr, success) = run(source);

    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "42\n");
    assert!(stderr.is_empty());
}

#[test]
fn reports_function_arity_errors_with_an_excerpt() {
    let (stdout, stderr, success) = run("fn identity(value) { value; }; identity();");

    assert!(!success);
    assert!(stdout.is_empty());
    assert!(stderr.contains("error: function expected 1 argument(s), received 0"));
    assert!(stderr.contains("identity();"));
}

#[test]
fn unterminated_strings_are_reported_with_an_excerpt() {
    let (_stdout, stderr, success) = run("\"oops");
    assert!(!success);
    assert!(stderr.contains("unterminated string literal"));
}

#[test]
fn non_boolean_conditions_are_runtime_errors() {
    let (_stdout, stderr, success) = run("if 1 + 1 { 2; };");
    assert!(!success);
    assert!(stderr.contains("condition of `if` must be a boolean"));
}

#[test]
fn evaluates_higher_order_functions_end_to_end() {
    let source = "
        fn apply(f, x) { f(x); };
        let twice = fn(n) { n * 2; };
        apply(twice, apply(fn(n) { n + 1; }, 19));
    ";
    let (stdout, stderr, success) = run(source);
    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "40\n");
    assert!(stderr.is_empty());
}

#[test]
fn return_outside_a_function_is_an_error() {
    let (_stdout, stderr, success) = run("return 5;");
    assert!(!success);
    assert!(stderr.contains("`return` outside of a function"));
}

#[test]
fn reports_runtime_errors_with_a_source_excerpt() {
    let (stdout, stderr, success) = run("1 / 0;");
    assert!(!success);
    assert!(stdout.is_empty());
    assert!(stderr.contains("error: division by zero"));
    assert!(stderr.contains("  1 | 1 / 0;"));
    assert!(stderr.contains("    | ^^^^^"));
}

#[test]
fn renders_each_line_of_a_multiline_error_span() {
    let (stdout, stderr, success) = run("1 /\n  0;");
    assert!(!success);
    assert!(stdout.is_empty());
    assert!(stderr.contains("  1 | 1 /"));
    assert!(stderr.contains("  2 |   0;"));
    assert!(stderr.contains("    | ^^^"));
    assert!(stderr.contains("    | ^^^"));
}

#[test]
fn ignores_comments_when_evaluating() {
    let (stdout, stderr, success) = run("// a comment\n2 + 3; // trailing\n");
    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "5\n");
    assert!(stderr.is_empty());
}

#[test]
fn runtime_errors_exit_with_code_one() {
    let (_stdout, stderr, code) = run_file_with_code("1 / 0;");
    assert_eq!(code, 1);
    assert!(stderr.contains("error: division by zero"));
}

#[test]
fn lexical_errors_stop_the_pipeline() {
    // `π` is a lexical error; the parser must never run on the remaining
    // garbage, so no downstream syntax diagnostics may appear.
    let (stdout, stderr, success) = run("let π = );");
    assert!(!success);
    assert!(stdout.is_empty());
    assert!(stderr.contains("unexpected character"));
    assert!(!stderr.contains("expected an expression"));
}

#[test]
fn syntax_errors_stop_the_pipeline_before_evaluation() {
    // The first statement would fail at runtime (`1 / 0`), but parsing the
    // second one fails first, so only the syntax error is reported.
    let (stdout, stderr, success) = run("1 / 0; let = ;");
    assert!(!success);
    assert!(stdout.is_empty());
    assert!(stderr.contains("expected an identifier after `let`"));
    assert!(!stderr.contains("division by zero"));
}

#[test]
fn help_flag_prints_usage_and_exits_zero() {
    let (stdout, stderr, code) = run_args(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("usage: ucl <file>"));
    assert!(stderr.is_empty());
}

#[test]
fn short_help_flag_is_equivalent() {
    let (stdout, _stderr, code) = run_args(&["-h"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("usage: ucl <file>"));
}

#[test]
fn version_flag_prints_the_version() {
    let (stdout, stderr, code) = run_args(&["--version"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), format!("ucl {}", env!("CARGO_PKG_VERSION")));
    assert!(stderr.is_empty());
}

#[test]
fn unknown_option_is_a_usage_error() {
    let (_stdout, stderr, code) = run_args(&["--bogus"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown option `--bogus`"));
    assert!(stderr.contains("usage: ucl <file>"));
}

#[test]
fn no_input_file_is_a_usage_error() {
    let (_stdout, stderr, code) = run_args(&[]);
    assert_eq!(code, 2);
    assert!(stderr.contains("no input file"));
}

#[test]
fn multiple_input_files_are_rejected() {
    let (_stdout, stderr, code) = run_args(&["a.ucl", "b.ucl"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("expected a single source file"));
}

#[test]
fn missing_file_is_reported() {
    let (_stdout, stderr, code) = run_args(&["definitely-missing.ucl"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("cannot read"));
}
