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
fn evaluates_len_end_to_end() {
    let (stdout, stderr, success) = run("len(\"hé\");");
    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "2\n");
    assert!(stderr.is_empty());
}

#[test]
fn reports_len_errors_without_stdout() {
    let (stdout, stderr, success) = run("len(1);");
    assert!(!success);
    assert!(stdout.is_empty());
    assert!(stderr.contains("`len` expects a string argument"));
}

#[test]
fn evaluates_new_builtins_end_to_end() {
    for (source, expected) in [
        ("str(42);", "42"),
        ("str(true);", "true"),
        ("str(\"hé\");", "hé"),
        ("type(1);", "integer"),
        ("upper(\"abc\");", "ABC"),
        ("lower(\"ABC\");", "abc"),
        ("contains(\"hello\", \"ell\");", "true"),
    ] {
        let (stdout, stderr, success) = run(source);
        assert!(success, "stderr: {stderr}");
        assert_eq!(stdout, format!("{expected}\n"), "for `{source}`");
        assert!(stderr.is_empty(), "for `{source}`");
    }
}

#[test]
fn str_mirrors_the_result_echo_for_callables() {
    let (stdout, stderr, success) = run("str(len);");
    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "<function>\n");

    // The same text the CLI would echo for a function value.
    let (stdout, _stderr, success) = run("fn f(n) { n; }; str(f);");
    assert!(success);
    assert_eq!(stdout, "<function>\n");
}

#[test]
fn str_renders_unit_as_its_type_name() {
    // A bare `return` produces unit; the echo omits unit but `str` renders it.
    let (stdout, stderr, success) = run("fn f() { return; }; str(f());");
    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "unit\n");
}

#[test]
fn reports_new_builtin_errors_without_stdout() {
    for source in ["upper(1);", "lower(true);", "contains(1, \"a\");"] {
        let (stdout, stderr, success) = run(source);
        assert!(!success, "for `{source}`");
        assert!(stdout.is_empty(), "for `{source}`");
        assert!(
            stderr.contains("expects a string"),
            "for `{source}`: {stderr}"
        );
    }
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
    assert!(stdout.contains("usage: ucl [<file>]"));
    assert!(stderr.is_empty());
}

#[test]
fn short_help_flag_is_equivalent() {
    let (stdout, _stderr, code) = run_args(&["-h"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("usage: ucl [<file>]"));
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
    assert!(stderr.contains("usage: ucl [<file>]"));
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

/// Feeds `input` to an interactive session and returns its stdout, stderr,
/// and exit code.
fn run_repl(input: &str) -> (String, String, i32) {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the ucl binary");
    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("write REPL input");

    let output = child.wait_with_output().expect("repl exits");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().expect("the process exited normally"),
    )
}

#[test]
fn repl_evaluates_and_echoes_expressions() {
    let (stdout, _stderr, code) = run_repl("1 + 2;\n\"hi\";\ntrue;\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("3"), "stdout: {stdout}");
    assert!(stdout.contains("hi"), "stdout: {stdout}");
    assert!(stdout.contains("true"), "stdout: {stdout}");
}

#[test]
fn repl_bindings_persist_across_lines() {
    let (stdout, _stderr, code) = run_repl("let x = 40;\nx + 2;\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("42"), "stdout: {stdout}");
}

#[test]
fn repl_definitions_span_multiple_lines() {
    let input = "fn make(base) {\n  return fn(n) {\n    base + n;\n  };\n};\nlet add5 = make(5);\nadd5(37);\n";
    let (stdout, _stderr, code) = run_repl(input);
    assert_eq!(code, 0);
    // The continuation prompt appears for each incomplete line, and the
    // closure created on one line is callable from a later one.
    assert!(stdout.contains("... "), "stdout: {stdout}");
    assert!(stdout.contains("42"), "stdout: {stdout}");
}

#[test]
fn repl_errors_do_not_end_the_session() {
    let (stdout, stderr, code) = run_repl("1 / 0;\n2 * 3;\n");
    assert_eq!(code, 0);
    assert!(stderr.contains("division by zero"), "stderr: {stderr}");
    assert!(stdout.contains("6"), "stdout: {stdout}");
}

#[test]
fn repl_reset_forgets_bindings() {
    let (stdout, stderr, code) = run_repl("let x = 1;\n:reset\nx;\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("session reset"), "stdout: {stdout}");
    assert!(stderr.contains("undefined variable"), "stderr: {stderr}");
}

#[test]
fn repl_reset_keeps_the_builtin_prelude() {
    let (stdout, stderr, code) = run_repl("let len = 1;\n:reset\nlen(\"abc\");\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("session reset"), "stdout: {stdout}");
    assert!(stdout.contains("3"), "stdout: {stdout}");
    assert!(stderr.is_empty(), "stderr: {stderr}");
}

#[test]
fn repl_reset_restores_every_builtin() {
    let (stdout, stderr, code) =
        run_repl("let str = 1;\nlet upper = 2;\n:reset\nstr(upper(\"a\"));\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("session reset"), "stdout: {stdout}");
    assert!(stdout.contains("A"), "stdout: {stdout}");
    assert!(stderr.is_empty(), "stderr: {stderr}");
}

#[test]
fn repl_quit_command_ends_the_session() {
    let (stdout, _stderr, code) = run_repl(":quit\n1 + 1;\n");
    assert_eq!(code, 0);
    // Exactly one prompt, and nothing evaluated after `:quit`.
    assert_eq!(
        stdout.matches(">>>").count(),
        1,
        ":quit must end the session immediately: {stdout}"
    );
    assert!(stdout.ends_with(">>> "), "no output after :quit: {stdout}");
}

#[test]
fn repl_incomplete_input_completes_across_lines() {
    // `let x = ` alone is incomplete; the next line completes the entry, so
    // no error is reported and later lines see the binding.
    let (stdout, stderr, code) = run_repl("let x = \n4;\nx + 1;\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("5"), "stdout: {stdout}");
    assert!(
        !stderr.contains("error"),
        "a completed entry must not report errors: {stderr}"
    );
}

fn programs_can_import_local_modules_setup() -> std::path::PathBuf {
    let dir = temp_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn programs_can_import_local_modules() {
    let dir = programs_can_import_local_modules_setup();
    fs::write(dir.join("math.ucl"), "fn double(n) { n * 2; };").expect("write module");
    let main_path = dir.join("main.ucl");
    fs::write(&main_path, "use \"math.ucl\";\ndouble(21);").expect("write main");

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg(&main_path)
        .output()
        .expect("run the ucl binary");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn programs_can_import_local_modules_with_a_namespace_alias() {
    let dir = programs_can_import_local_modules_setup();
    fs::write(
        dir.join("math.ucl"),
        "fn double(n) { n * 2; }; let answer = 21;",
    )
    .expect("write module");
    let main_path = dir.join("main.ucl");
    fs::write(
        &main_path,
        "use \"math.ucl\" as math;\nmath.double(math.answer);",
    )
    .expect("write main");

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg(&main_path)
        .output()
        .expect("run the ucl binary");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_circular_import_is_reported_with_an_excerpt() {
    let dir = programs_can_import_local_modules_setup();
    fs::write(dir.join("a.ucl"), "use \"b.ucl\";").expect("write a");
    fs::write(dir.join("b.ucl"), "use \"a.ucl\";").expect("write b");
    let main_path = dir.join("main.ucl");
    fs::write(&main_path, "use \"a.ucl\";\n1;").expect("write main");

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg(&main_path)
        .output()
        .expect("run the ucl binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("circular import"), "stderr: {stderr}");
    assert!(stderr.contains("-->"), "an excerpt is rendered: {stderr}");
    let _ = fs::remove_dir_all(&dir);
}

/// Returns a unique temporary directory path (not yet created).
fn temp_dir() -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ucl-cli-dir-{}-{id}", std::process::id()))
}

#[test]
fn repl_use_resolves_modules_against_the_working_directory() {
    use std::io::Write;
    use std::process::Stdio;

    let dir = temp_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(dir.join("helper.ucl"), "let magic = 99;").expect("write module");

    let mut child = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the ucl binary");
    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(b"use \"helper.ucl\" as helper;\nhelper.magic;\n")
        .expect("write REPL input");
    let output = child.wait_with_output().expect("repl exits");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("99"), "stdout: {stdout}");
    let _ = fs::remove_dir_all(&dir);
}
