# Development guide

This guide covers the local workflow and contribution expectations for UCL.
For planned work, see the [project roadmap](roadmap.md). Language behavior is
defined by the [language specification](spec.md).

## Prerequisites

Install the stable Rust toolchain with Cargo. The repository's
`rust-toolchain.toml` requests the required `rustfmt` and Clippy components.

## Local workflow

Build and run the interpreter:

```sh
cargo build
cargo run -- example.ucl
cargo build --release --locked
```

Run the same core checks as CI before submitting a change:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

The CI workflow runs formatting, linting, tests, and a locked release build.

## Project architecture

The interpreter uses a staged pipeline:

```text
source text -> lexer -> parser -> evaluator
```

- `source.rs` owns source text and byte spans.
- `diagnostic.rs` collects structured errors, warnings, and notes.
- `lexer.rs` converts source text into tokens.
- `parser.rs` converts tokens into an abstract syntax tree.
- `evaluator.rs` executes the abstract syntax tree.
- `main.rs` provides the `ucl` command-line interface and diagnostic rendering.

## Contributing

Bug reports and focused pull requests are welcome.

When changing behavior:

1. Keep the implementation aligned with `docs/spec.md`.
2. Add or update tests that demonstrate the expected behavior.
3. Run the formatting, lint, and test commands above.
4. Update documentation when syntax, semantics, or public APIs change.

Language changes must update `docs/spec.md` in the same pull request so the
implementation and specification remain synchronized.

## Licensing contributions

Unless explicitly stated otherwise, contributions intentionally submitted for
inclusion in UCL are licensed under either Apache-2.0 or MIT, at the recipient's
option, without additional conditions.
