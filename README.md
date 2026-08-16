# Universal Coding Language (UCL)

A universal coding language that excels in every task possible.

> **Status:** early scaffold. The project layout, tooling, and CI are in
> place; the lexer, parser, and evaluator are implemented and wired together
> by the `ucl` CLI.

## Repository layout

```
.
├── src/
│   ├── main.rs        # `ucl` CLI entry point
│   ├── lib.rs         # Library root and public API
│   ├── source.rs      # Source files, byte positions, and spans
│   ├── diagnostic.rs  # Structured errors, warnings, and notes
│   ├── lexer.rs       # Source text → tokens
│   ├── parser.rs      # Tokens → AST
│   └── evaluator.rs   # AST → values
├── tests/
│   └── smoke.rs       # Integration tests for the public API
├── .github/workflows/ # CI (format, lint, test, release build)
└── Cargo.toml
```

## Requirements

- Rust stable (see `rust-toolchain.toml`)

## Building and running

```sh
cargo build            # debug build
cargo run -- <file>    # evaluate a `.ucl` source file
cargo build --release  # optimized build
```

## Development

```sh
cargo fmt --all -- --check                             # check formatting
cargo clippy --all-targets --all-features -- -D warnings  # lint
cargo test                                             # run tests
# Note: cargo test is deligated to ci (github action)
```

## Roadmap

- [x] Write the [language specification](docs/spec.md)
- [x] Implement the lexer
- [x] Implement the parser and AST
- [x] Implement the evaluator
- [x] Wire up the `ucl` CLI
- [x] Add diagnostics rendering with source excerpts

## License

TBD.
