# Changelog

All notable changes to UCL are documented here.

## 0.2.0 - 2026-08-22

### Added

- Boolean literals: `true` and `false` are now reserved keywords and can be
  used as expressions.
- Relational operators `<=` and `>=`, complementing `<` and `>`.
- Equality operators `==` and `!=`, defined for two integers or two booleans.
- The logical operators `&` and `|` now short-circuit: the right-hand side is
  not evaluated when its value cannot change the result.

### Changed

- **Breaking (library API):** `Evaluator::evaluate` now returns
  `Option<Value>` — `None` when runtime errors occurred during evaluation,
  `Some(value)` otherwise. Previously it returned the last successfully
  computed value even after errors.
- **Breaking (language):** a program with lexical or syntax errors is no
  longer executed; previously, statements before the error could still run.
  On any pipeline error no value is produced.
- **Breaking (library API):** `AstKind::Binary` carries a `BinaryOperator`
  instead of a bare `char`.
- The parser's nesting limit (256) and the evaluator's nesting guard (512)
  are now consistent: every program the parser accepts evaluates. Binary
  operator chains of arbitrary length are supported.

### Fixed

- Hand-built ASTs with invalid spans produce diagnostics instead of panics.

## 0.1.0 - 2026-08-22

Initial experimental release. Syntax and behavior are unstable and may change
in any release; see the [language specification](docs/spec.md) for what is
currently supported.

### Added

- Lexer, parser, and evaluator for the language described in `docs/spec.md`
- Signed 64-bit integers and booleans with checked arithmetic
- `let` declarations, assignment, and blocks with lexical scoping
- Line comments beginning with `//`
- Diagnostics rendered with source excerpts
- The `ucl` command-line interface
- Prebuilt Linux x86_64 binaries attached to GitHub releases
