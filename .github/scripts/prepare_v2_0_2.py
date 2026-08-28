from pathlib import Path


def replace(path: str, old: str, new: str, count: int = 1) -> None:
    file = Path(path)
    text = file.read_text()
    if old not in text:
        raise SystemExit(f"expected text not found in {path}: {old!r}")
    file.write_text(text.replace(old, new, count))


replace(
    "src/evaluator/typecheck.rs",
    '''        if self.strict && !has_annotation {
            self.error(
                node.span,
                "type error: `--strict-types` requires an annotated function signature",
            );
        }
''',
    '''        let has_complete_signature = return_type.is_some()
            && parameters
                .iter()
                .all(|parameter| parameter.annotation.is_some());
        if self.strict && !has_complete_signature {
            self.error(
                node.span,
                "type error: `--strict-types` requires every function parameter and return type to be annotated",
            );
        }
''',
)

replace(
    "tests/cli.rs",
    'stderr.contains("--strict-types` requires an annotated function signature")',
    'stderr.contains("--strict-types` requires every function parameter and return type to be annotated")',
)

cli = Path("tests/cli.rs")
cli.write_text(
    cli.read_text()
    + '''

#[test]
fn strict_types_rejects_partially_annotated_function_signatures() {
    for program in [
        "fn identity(value: int) { value; };",
        "fn add(left: int, right): int { left; };",
        "fn add(left, right: int): int { right; };",
    ] {
        let (stdout, stderr, code) = run_args(&["--strict-types", "-e", program]);
        assert_eq!(code, 1, "program unexpectedly succeeded: {program}");
        assert!(stdout.is_empty());
        assert!(
            stderr.contains("requires every function parameter and return type to be annotated"),
            "stderr for {program}: {stderr}"
        );
    }
}
'''
)

replace("Cargo.toml", 'version = "2.0.1"', 'version = "2.0.2"')
replace(
    "Cargo.lock",
    'name = "universal-coding-language"\nversion = "2.0.1"',
    'name = "universal-coding-language"\nversion = "2.0.2"',
)
replace(
    "fuzz/Cargo.lock",
    'name = "universal-coding-language"\nversion = "2.0.1"',
    'name = "universal-coding-language"\nversion = "2.0.2"',
)

changelog = Path("CHANGELOG.md")
marker = "All notable changes to UCL are documented here.\n\n"
entry = """## 2.0.2 - 2026-08-28

### Fixed

- `--strict-types` now enforces its documented complete-signature contract: every function parameter and the return type must be annotated. Previously, a single annotation was enough to let a partially typed function pass strict mode.
- Added end-to-end CLI regression coverage for missing return annotations and mixed annotated/unannotated parameter lists.

"""
text = changelog.read_text()
if marker not in text:
    raise SystemExit("CHANGELOG insertion marker not found")
changelog.write_text(text.replace(marker, marker + entry, 1))

replace(
    "README.md",
    "UCL 2.0.1 is the current stable release.",
    "UCL 2.0.2 is the current stable release.",
)
replace(
    "README.md",
    "This patch also ensures static checking honors user bindings that shadow\n> built-in function names.",
    "This patch enforces complete function signatures in `--strict-types`;\n> the 2.0.1 built-in-shadowing fix remains included.",
)
replace("docs/guarantees.md", "**Active version:** `2.0.1`", "**Active version:** `2.0.2`")
replace(
    "docs/guarantees.md",
    "with `2.0.1` as the current patch release",
    "with `2.0.2` as the current patch release",
)
replace("docs/v2-goal.md", "**Current v2 release:** `2.0.1`", "**Current v2 release:** `2.0.2`")
replace(
    "docs/roadmap.md",
    "**UCL 2.0.1** is the current maintenance release",
    "**UCL 2.0.2** is the current maintenance release",
)
