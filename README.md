# Universal Coding Language (UCL)

A universal coding language that excels in every task possible.

UCL is currently implemented as an experimental, expression-oriented language
in Rust. The project provides a small interpreter with a lexer, parser,
evaluator, command-line interface, and source-aware diagnostics.

> [!IMPORTANT]
> UCL is an early-stage project, not yet a general-purpose or production-ready
> language. Syntax and public Rust APIs may change without notice.

## Current features

- Signed 64-bit integers and booleans
- Checked arithmetic and comparison operators
- `let` declarations and assignment
- Blocks with lexical scoping
- Line comments beginning with `//`
- Error diagnostics with source excerpts
- A library API and the `ucl` command-line program

The implemented language is defined in the [language specification](docs/spec.md).
Features not described there should be treated as unsupported.

## Example

Create `example.ucl`:

```ucl
let width = 6;
let height = 7;
width * height;
```

Evaluate it:

```console
$ cargo run -- example.ucl
42
```

Use `cargo run -- --help` to display command-line help.

## Requirements

- Rust stable with Cargo (see `rust-toolchain.toml`)

## Build and test

```sh
cargo build
cargo test --all-features
cargo run -- example.ucl
cargo build --release --locked
```

Before submitting a change, run the same core checks as CI:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

## Repository layout

```text
.
├── docs/
│   └── spec.md          # Implemented language syntax and semantics
├── src/
│   ├── main.rs          # `ucl` CLI
│   ├── lib.rs           # Public Rust API
│   ├── source.rs        # Source files, byte positions, and spans
│   ├── diagnostic.rs    # Structured diagnostics
│   ├── lexer.rs         # Source text to tokens
│   ├── parser.rs        # Tokens to AST
│   └── evaluator.rs     # AST evaluation
├── tests/
│   ├── cli.rs           # End-to-end CLI tests
│   └── smoke.rs         # Public API smoke tests
└── .github/workflows/ci.yml
```

## Roadmap

The roadmap prioritizes correctness and a stable language foundation before
adding major features.

### Near term: correctness and resilience

- [ ] Recognize `let` as an actual keyword instead of accepting any identifier
      in the declaration position
- [ ] Stop evaluation when lexing or parsing reports errors
- [ ] Make evaluator failures explicit rather than representing them as `unit`
- [ ] Validate source spans and prevent invalid public spans from panicking
- [ ] Add regression tests for overflow, malformed syntax, UTF-8 diagnostics,
      nested scopes, and CLI argument handling
- [ ] Add parser and evaluator nesting limits

### Language foundation

- [ ] Define keyword, assignment, operator, and runtime-error semantics
- [ ] Decide whether UCL will be statically or dynamically typed
- [ ] Add boolean literals and equality operators
- [ ] Add strings and string operations
- [ ] Add functions, parameters, return values, and closures
- [ ] Add conditional and looping constructs

### Tooling and ecosystem

- [ ] Add property tests and fuzz targets for the lexer and parser
- [ ] Publish diagnostic and compatibility guarantees
- [ ] Design a module and package system
- [ ] Add an interactive REPL
- [ ] Provide release artifacts and installation instructions

Completed foundation work includes the initial specification, lexer, parser,
evaluator, CLI, diagnostics renderer, integration tests, and CI checks.

## Contributing

Bug reports and focused pull requests are welcome. Please keep changes aligned
with the implemented specification, include tests for behavioral changes, and
run the formatting, lint, and test commands above.

For language changes, update `docs/spec.md` in the same pull request so the
implementation and specification remain synchronized.

## License

Licensed under either of the following, at your option:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project is licensed under those terms, without additional
conditions.
