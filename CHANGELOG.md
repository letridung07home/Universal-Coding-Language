# Changelog

All notable changes to UCL are documented here.

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
