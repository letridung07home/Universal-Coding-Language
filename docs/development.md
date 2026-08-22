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

The CI workflow runs formatting, linting, tests, a warning-free documentation
build, a fuzz-target compilation check, and a locked release build.

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
- `module.rs` loads `use` imports: path resolution, cycle detection,
  evaluation isolation, and binding merges.
- `render.rs` renders diagnostics with source excerpts.
- `repl.rs` runs interactive sessions with a persistent environment.
- `main.rs` provides the `ucl` command-line interface.

## Property testing and fuzzing

In addition to the unit and integration tests, the repository ships
dependency-free property tests in `tests/property.rs`. They use a deterministic
pseudo-random generator, so a failure can be reproduced exactly:

```sh
cargo test --test property
```

The property tests assert that the pipeline never panics on arbitrary input,
that the lexer always emits a trailing `Eof` token and valid spans, and that
integer arithmetic and operator precedence agree with Rust's own checked
arithmetic and the language specification.

Fuzz targets for the lexer, parser, and full pipeline live in `fuzz/` and use
[`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz), which requires the
nightly toolchain and the `cargo-fuzz` subcommand:

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run lexer
cargo +nightly fuzz run parser
cargo +nightly fuzz run pipeline
```

`fuzz/` is a standalone crate and is not built by the normal `cargo build`,
`cargo test`, or `cargo clippy` invocations at the repository root. The CI
workflow compiles the fuzz targets on every push, and the separate **Fuzz**
workflow runs them nightly for ten minutes per target (four parallel workers
each); it can also be triggered manually from the Actions tab, where the
per-target budget is configurable and defaults to one minute.

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
