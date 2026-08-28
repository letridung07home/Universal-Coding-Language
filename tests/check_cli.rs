//! End-to-end coverage for the `ucl check` batch checker.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("ucl-check-{}-{id}", std::process::id()));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

fn run(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_ucl"))
        .args(args)
        .output()
        .expect("run ucl");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().expect("normal process exit"),
    )
}

#[test]
fn checks_multiple_entry_files_without_evaluating_them() {
    let dir = temp_dir();
    let first = dir.join("first.ucl");
    let second = dir.join("second.ucl");
    fs::write(&first, "let x: int = 1; x;").expect("write first source");
    fs::write(&second, "let y: string = \"ok\"; y;").expect("write second source");

    let (stdout, stderr, code) = run(&[
        "check",
        first.to_str().expect("utf-8 path"),
        second.to_str().expect("utf-8 path"),
    ]);

    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn reports_type_errors_in_any_entry_file() {
    let dir = temp_dir();
    let good = dir.join("good.ucl");
    let bad = dir.join("bad.ucl");
    fs::write(&good, "let x: int = 1;").expect("write good source");
    fs::write(&bad, "let x: int = true;").expect("write bad source");

    let (stdout, stderr, code) = run(&[
        "check",
        good.to_str().expect("utf-8 path"),
        bad.to_str().expect("utf-8 path"),
    ]);

    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("initializer expects `int`"), "stderr: {stderr}");
    assert!(stderr.contains("bad.ucl"), "stderr: {stderr}");
}

#[test]
fn strict_batch_check_requires_complete_signatures() {
    let dir = temp_dir();
    let source = dir.join("strict.ucl");
    fs::write(&source, "fn identity(value) { value; };").expect("write source");

    let (stdout, stderr, code) = run(&[
        "check",
        "--strict-types",
        source.to_str().expect("utf-8 path"),
    ]);

    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("requires every function parameter and return type to be annotated"));
}

#[test]
fn checks_imported_modules_without_running_them() {
    let dir = temp_dir();
    let root = dir.join("app.ucl");
    let module = dir.join("module.ucl");
    fs::write(&root, "use \"module\" as module; let ok: int = 1;").expect("write root");
    fs::write(&module, "let broken: int = false;").expect("write module");

    let (stdout, stderr, code) = run(&["check", root.to_str().expect("utf-8 path")]);

    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("initializer expects `int`"), "stderr: {stderr}");
    assert!(stderr.contains("module.ucl"), "stderr: {stderr}");
}

#[test]
fn check_requires_at_least_one_file() {
    let (stdout, stderr, code) = run(&["check"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("expects at least one source file"));
}
