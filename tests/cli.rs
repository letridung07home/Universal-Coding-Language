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

/// Pipes `source` to the `ucl` binary's stdin along with the given arguments,
/// and returns its stdout, stderr, and exit code.
///
/// Under parallel test load a freshly spawned child can exit before
/// consuming piped input, surfacing as a broken pipe on our first write.
/// That race is environmental (an exec-time hiccup in the child), not a
/// property of `ucl -`, so the whole invocation is retried; the assertions
/// still judge the final attempt's real behavior.
fn run_stdin(source: &str, args: &[&str]) -> (String, String, i32) {
    use std::io::Write;
    use std::process::Stdio;

    for attempt in 0..3 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ucl"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the ucl binary");
        let write_result = child
            .stdin
            .as_mut()
            .expect("stdin is piped")
            .write_all(source.as_bytes());
        let broken_pipe =
            matches!(&write_result, Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe);
        let output = child.wait_with_output().expect("run the ucl binary");
        if broken_pipe && attempt < 2 {
            continue;
        }
        write_result.expect("write program text to stdin");

        return (
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            output.status.code().expect("the process exited normally"),
        );
    }
    unreachable!("the loop returns on its final attempt")
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
    assert!(stderr.contains("`len` expects a string or list argument"));
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
    for source in [
        "upper(1);",
        "lower(true);",
        "contains(1, \"a\");",
        "find(\"a\", 1);",
        "replace(1, \"b\", \"c\");",
        "trim(1);",
        "slice(1, 0, 1);",
    ] {
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
fn search_path_flag_resolves_imports() {
    let app = temp_dir();
    let lib = temp_dir();
    fs::create_dir_all(&app).expect("create app dir");
    fs::create_dir_all(&lib).expect("create lib dir");
    fs::write(lib.join("math.ucl"), "fn double(n) { n * 2; };").expect("write module");
    let main = app.join("main.ucl");
    fs::write(&main, "use \"math\" as m; m.double(21);").expect("write main");

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg("-p")
        .arg(&lib)
        .arg(&main)
        .output()
        .expect("run the ucl binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");

    // Without the flag the import cannot resolve.
    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg(&main)
        .env_remove("UCL_PATH")
        .output()
        .expect("run the ucl binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("none of these locations exist"), "{stderr}");

    let _ = fs::remove_dir_all(app);
    let _ = fs::remove_dir_all(lib);
}

#[test]
fn list_imports_reports_resolved_transitive_graph_without_evaluating_source() {
    let app = temp_dir();
    let lib = temp_dir();
    fs::create_dir_all(app.join("tools")).expect("create app directories");
    fs::create_dir_all(&lib).expect("create library directory");
    fs::write(lib.join("math.ucl"), "use \"helper\"; let ignored = 1 / 0;")
        .expect("write math module");
    fs::write(lib.join("helper.ucl"), "let answer = 42;").expect("write helper module");
    fs::write(app.join("tools/format.ucl"), "let formatter = 1;").expect("write local module");
    let main = app.join("main.ucl");
    fs::write(
        &main,
        "let ignored = 1 / 0; use \"math\"; use \"tools/format\";",
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg("--list-imports")
        .arg("-p")
        .arg(&lib)
        .arg(&main)
        .output()
        .expect("run the ucl binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = main.canonicalize().expect("canonical main path");
    let math = lib
        .join("math.ucl")
        .canonicalize()
        .expect("canonical math path");
    let helper = lib
        .join("helper.ucl")
        .canonicalize()
        .expect("canonical helper path");
    let formatter = app
        .join("tools/format.ucl")
        .canonicalize()
        .expect("canonical formatter path");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!(
            "{}\n{} -> {}\n{} -> {}\n{} -> {}\n",
            root.display(),
            root.display(),
            math.display(),
            math.display(),
            helper.display(),
            root.display(),
            formatter.display(),
        )
    );

    let _ = fs::remove_dir_all(app);
    let _ = fs::remove_dir_all(lib);
}

#[test]
fn list_imports_reports_resolution_failures_and_rejects_invalid_invocations() {
    let app = temp_dir();
    fs::create_dir_all(&app).expect("create app directory");
    let main = app.join("main.ucl");
    fs::write(&main, "use \"missing\";").expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg("--list-imports")
        .arg(&main)
        .output()
        .expect("run the ucl binary");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("none of these locations exist"));

    let (stdout, stderr, code) = run_args(&["--list-imports"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("`--list-imports` requires a source file"));

    let (stdout, stderr, code) = run_args(&[
        "--list-imports",
        "--list-imports",
        &main.display().to_string(),
    ]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("repeated `--list-imports` flag"));

    let _ = fs::remove_dir_all(app);
}

#[test]
fn evaluates_break_and_continue_end_to_end() {
    let source = "
        let evens = 0;
        let i = 0;
        while true {
            i = i + 1;
            if i > 10 { break; };
            if i % 2 == 1 { continue; };
            evens = evens + 1;
        };
        evens;
    ";
    let (stdout, stderr, success) = run(source);
    assert!(success, "stderr: {stderr}");
    assert_eq!(stdout, "5\n");
    assert!(stderr.is_empty());
}

#[test]
fn break_outside_a_loop_reports_an_error() {
    for source in ["break;", "fn f() { break; }; f();"] {
        let (_stdout, stderr, success) = run(source);
        assert!(!success, "for `{source}`");
        assert!(stderr.contains("`break` outside of a loop"), "{stderr}");
    }
}

#[test]
fn eval_flag_runs_inline_program_text() {
    let (stdout, stderr, code) = run_args(&["-e", "let x = 6 * 7; x;"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
    assert!(stderr.is_empty());

    let (stdout, stderr, code) = run_args(&["--eval", "upper(\"ucl\");"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "UCL\n");
    assert!(stderr.is_empty());
}

#[test]
fn eval_flag_reports_runtime_errors_against_the_eval_source() {
    let (stdout, stderr, code) = run_args(&["-e", "1 / 0;"]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("error: division by zero"));
    assert!(stderr.contains("<eval>:1:1"));
}

#[test]
fn eval_flag_resolves_imports_through_path_flags() {
    let lib = temp_dir();
    fs::create_dir_all(&lib).expect("create lib dir");
    fs::write(lib.join("helper.ucl"), "let answer = 40;").expect("write module");

    let lib_string = lib.display().to_string();
    let (stdout, stderr, code) =
        run_args(&["-p", &lib_string, "-e", "use \"helper\"; answer + 2;"]);

    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "42\n");

    let _ = fs::remove_dir_all(lib);
}

#[test]
fn eval_flag_combinations_are_usage_errors() {
    let source = temp_path();
    fs::write(&source, "1;").expect("write source file");

    let (stdout, stderr, code) = run_args(&["-e", "2;", "-e", "3;"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("expected a single `--eval` program"));

    let source_string = source.display().to_string();
    let (stdout, stderr, code) = run_args(&["-e", "2;", &source_string]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("cannot combine `--eval` with a source file"));

    // A missing `-e` value is also a usage error.
    let (_stdout, stderr, code) = run_args(&["-e"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("`-e` requires program text"));

    let _ = fs::remove_file(source);
}

#[test]
fn dash_reads_the_program_from_piped_stdin() {
    let (stdout, stderr, code) = run_stdin("2 + 3 * 4;", &["-"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "14\n");
    assert!(stderr.is_empty());

    let (stdout, stderr, code) = run_stdin("int(\"40\") + 2;", &["-"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "42\n");
    assert!(stderr.is_empty());

    // Runtime errors still report with the `<stdin>` excerpt.
    let (stdout, stderr, code) = run_stdin("1 / 0;", &["-"]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("error: division by zero"));
    assert!(stderr.contains("<stdin>:1:1"));

    // `-` cannot share the command line with `--eval` or another file.
    let (stdout, stderr, code) = run_stdin("1;", &["-e", "2;", "-"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("cannot combine `--eval` with standard input"));
}

#[test]
fn ucl_path_environment_resolves_imports() {
    let app = temp_dir();
    let lib = temp_dir();
    fs::create_dir_all(&app).expect("create app dir");
    fs::create_dir_all(&lib).expect("create lib dir");
    fs::write(lib.join("helper.ucl"), "let answer = 40;").expect("write module");
    let main = app.join("main.ucl");
    fs::write(&main, "use \"helper\"; answer + 2;").expect("write main");

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .arg(&main)
        .env("UCL_PATH", &lib)
        .output()
        .expect("run the ucl binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");

    let _ = fs::remove_dir_all(app);
    let _ = fs::remove_dir_all(lib);
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
fn for_loop_programs_run_end_to_end() {
    let (stdout, _stderr, code) =
        run_file_with_code("let total = 0; for i in 1..5 { total = total + i; }; total;");
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "10");
}

#[test]
fn list_programs_run_end_to_end() {
    let (stdout, _stderr, code) =
        run_file_with_code("let items = [\"x\", \"y\", \"z\"]; len(items);");
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "3");

    let (stdout, _stderr, code) =
        run_file_with_code("for word in [\"a\", \"b\"] { word; }; \"done\";");
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "done");

    let (_stdout, stderr, code) = run_file_with_code("[1, 2][9];");
    assert_eq!(code, 1);
    assert!(stderr.contains("`index` is out of range"));
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
    assert!(stdout.contains("usage: ucl [-p <dir>]... [-e <code> | <file>]"));
    assert!(stdout.contains("-e, --eval <code>"));
    assert!(stderr.is_empty());
}

#[test]
fn short_help_flag_is_equivalent() {
    let (stdout, _stderr, code) = run_args(&["-h"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("usage: ucl [-p <dir>]... [-e <code> | <file>]"));
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
    assert!(stderr.contains("usage: ucl [-p <dir>]... [-e <code> | <file>]"));
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

// ---------------------------------------------------------------------------
// ucl fmt: the source formatter subcommand
// ---------------------------------------------------------------------------

/// Writes `source` to a temporary file and runs `ucl fmt` with `args`
/// (the path is appended). Returns stdout, stderr, exit code, and the
/// file's contents afterward.
fn run_fmt(source: &str, args: &[&str]) -> (String, String, i32, String) {
    let path = temp_path();
    fs::write(&path, source).expect("write source file");
    let mut argv = vec!["fmt"];
    argv.extend_from_slice(args);
    let path_string = path.to_string_lossy().into_owned();
    argv.push(&path_string);

    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .args(&argv)
        .output()
        .expect("run the ucl binary");
    let contents = fs::read_to_string(&path).unwrap_or_default();
    let _ = fs::remove_file(&path);

    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().expect("the process exited normally"),
        contents,
    )
}

#[test]
fn fmt_rewrites_a_file_in_place() {
    let (_, _, code, contents) = run_fmt("let   x=1;\nif x>1 { y=2;};", &[]);
    assert_eq!(code, 0);
    assert_eq!(contents, "let x = 1;\nif x > 1 {\n    y = 2;\n};\n");
}

#[test]
fn fmt_leaves_formatted_files_untouched() {
    let formatted = "let x = 1;\n";
    let (_, _, code, contents) = run_fmt(formatted, &[]);
    assert_eq!(code, 0);
    assert_eq!(contents, formatted);
}

#[test]
fn fmt_check_reports_unformatted_files_without_touching_them() {
    let messy = "let   x=1;";
    let (stdout, _, code, contents) = run_fmt(messy, &["--check"]);
    assert_eq!(code, 1);
    assert!(stdout.contains("not formatted"), "stdout: {stdout}");
    assert_eq!(contents, messy, "--check must not rewrite the file");
}

#[test]
fn fmt_check_exits_zero_for_formatted_files() {
    let (_, _, code, _) = run_fmt("let x = 1;\n", &["--check"]);
    assert_eq!(code, 0);
}

#[test]
fn fmt_pipes_stdin_to_stdout() {
    let (stdout, _, code) = run_stdin("fn f( a){\n return a;};\nf(1);", &["fmt", "-"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "fn f(a) {\n    return a;\n};\nf(1);\n");
}

#[test]
fn fmt_preserves_comments_end_to_end() {
    let source = "// header\nlet a = 1; // trailing\n/* block\n   comment */\nlet b = 2;";
    let (_, _, _, contents) = run_fmt(source, &[]);
    assert!(contents.contains("// header"), "{contents}");
    assert!(contents.contains("let a = 1; // trailing"), "{contents}");
    assert!(contents.contains("/* block\n   comment */"), "{contents}");
}

#[test]
fn fmt_never_touches_sources_with_errors() {
    let broken = "let = ;";
    let (_, _, code, contents) = run_fmt(broken, &[]);
    assert_eq!(code, 1);
    assert_eq!(contents, broken, "a broken file must stay untouched");
}

#[test]
fn fmt_usage_errors_exit_two() {
    // No input at all.
    let (_, stderr, code) = run_args(&["fmt"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("expects a file"), "{stderr}");
    // Unknown flag.
    let (_, _, code, _) = run_fmt("1;", &["--wat"]);
    assert_eq!(code, 2);
}

#[test]
fn type_check_mode_validates_annotations_without_running_the_program() {
    let path = temp_path();
    fs::write(&path, "let answer: int = 42; 1 / 0;").expect("write source file");

    let (stdout, stderr, code) = run_args(&["--type-check", path.to_str().expect("utf-8 path")]);
    let _ = fs::remove_file(&path);

    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn type_check_mode_reports_annotated_mismatches_before_evaluation() {
    let (stdout, stderr, code) = run_args(&["--type-check", "-e", "let answer: int = true;"]);

    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("type error: initializer expects `int`, found `bool`"));
}

#[test]
fn type_checking_honors_user_bindings_that_shadow_builtins() {
    for (program, expected) in [
        (
            "fn upper(value: int): int { value + 1; }; let result: int = upper(41); result;",
            "42\n",
        ),
        (
            "let upper: function = fn(value) { value + 1; }; let result: int = upper(41); result;",
            "42\n",
        ),
    ] {
        let (stdout, stderr, code) = run_args(&["--type-check", "-e", program]);
        assert_eq!(code, 0, "stderr: {stderr}; program: {program}");
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());

        let (stdout, stderr, code) = run_args(&["-e", program]);
        assert_eq!(code, 0, "stderr: {stderr}; program: {program}");
        assert_eq!(stdout, expected, "program: {program}");
        assert!(stderr.is_empty());
    }
}

#[test]
fn strict_types_requires_function_signatures_and_evaluates_valid_programs() {
    let (stdout, stderr, code) = run_args(&["--strict-types", "-e", "fn id(x) { x; }; id(1);"]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("--strict-types` requires an annotated function signature"));

    let (stdout, stderr, code) = run_args(&[
        "--strict-types",
        "-e",
        "fn twice(x: int): int { x + x; }; twice(21);",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "42\n");
    assert!(stderr.is_empty());
}

#[test]
fn inspection_and_type_checking_modes_cannot_be_combined() {
    let (stdout, stderr, code) = run_args(&["--list-imports", "--type-check", "-e", "1;"]);

    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("cannot combine `--list-imports` with type-checking flags"));
}
