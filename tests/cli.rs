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
