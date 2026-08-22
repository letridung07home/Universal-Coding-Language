# Universal Coding Language (UCL)

A universal coding language that excels in every task possible.

UCL is currently implemented as an experimental, expression-oriented language
in Rust. The project provides a small interpreter with a lexer, parser,
evaluator, command-line interface, and source-aware diagnostics.

> [!IMPORTANT]
> UCL is an early-stage project, not yet a general-purpose or production-ready
> language. Syntax and public Rust APIs may change without notice.

## Current features

- Signed 64-bit integers, booleans, and strings
- Checked arithmetic, comparison, and equality operators
- String concatenation with escape sequences
- Short-circuiting logical operators
- `if`/`else` conditionals and `while` loops
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
let label = "area";
if width * height == 42 { label; } else { "mismatch"; };
```

Evaluate it:

```console
$ cargo run -- example.ucl
area
```

Use `cargo run -- --help` to display command-line help.

## Requirements

- Rust stable with Cargo (see `rust-toolchain.toml`) — only needed when
  building from source; prebuilt binaries are available

## Install

Download a prebuilt Linux binary from the [latest release](https://github.com/letridung07home/Universal-Coding-Language/releases/latest):

```sh
curl -LO https://github.com/letridung07home/Universal-Coding-Language/releases/latest/download/ucl-x86_64-linux.tar.gz
tar -xzf ucl-x86_64-linux.tar.gz
sudo mv ucl-x86_64-linux/ucl /usr/local/bin/
```

Verify the download against `sha256sums.txt` attached to the same release.

## Build and run

```sh
cargo build
cargo run -- example.ucl
cargo build --release --locked
```

See the [development guide](docs/development.md) for testing, linting,
architecture, and contribution instructions.

## Repository layout

```text
.
├── docs/
│   ├── development.md   # Development workflow and contribution guide
│   ├── roadmap.md       # Planned milestones and project direction
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

Current priorities are interpreter correctness, stable language semantics, and
stronger testing. See the [project roadmap](docs/roadmap.md) for planned
milestones.

## Contributing

See the [development guide](docs/development.md) for the local workflow,
architecture overview, testing requirements, and contribution guidance.

## License

Licensed under either of the following, at your option:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project is licensed under those terms, without additional
conditions.
