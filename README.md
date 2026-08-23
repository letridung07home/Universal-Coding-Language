# Universal Coding Language (UCL)

A universal coding language that excels in every task possible.

UCL is a small, expression-oriented language implemented in Rust. The project
provides an interpreter with a lexer, parser, evaluator, file-based modules,
an interactive REPL, a command-line interface, and source-aware diagnostics.

> [!NOTE]
> UCL 1.4 follows the first stable release: the language, CLI, library API, and
> error categories are covered by [compatibility guarantees](docs/guarantees.md).
> It remains a deliberately small language — not a batteries-included
> general-purpose scripting environment.

## Features

- Signed 64-bit integers, booleans, and strings
- Checked arithmetic, comparison, and equality operators
- String concatenation with escape sequences and a deterministic 8 MiB value limit
- Built-in functions: `len(string)`, `str(value)`, `type(value)`,
  `upper(string)`, `lower(string)`, and `contains(haystack, needle)`
- Short-circuiting logical operators
- `if`/`else` conditionals and `while` loops
- Functions: declarations, literals, closures, and recursion
- `let` declarations and assignment
- Blocks with lexical scoping
- File-based modules via legacy flat imports or read-only namespaces: `use "path.ucl";` and `use "path.ucl" as math;`
- Line comments (`//`) and nesting block comments (`/* */`)
- String comparisons (`<`, `<=`, `>`, `>=`)
- Error diagnostics with source excerpts
- A library API and the `ucl` command-line program

The language is defined in the [language specification](docs/spec.md).
What the project keeps stable across releases — language, diagnostics, CLI,
and library API — is described in the
[compatibility guarantees](docs/guarantees.md).
Features not described there should be treated as unsupported.

## Example

Create `example.ucl`:

```ucl
fn area(w, h) { w * h; };
let describe = fn(n) { if n == 42 { "the answer"; } else { "not quite"; }; };
let label = upper("ucl");
label + ": " + str(contains(label, "UCL"));
```

Evaluate it:

```console
$ cargo run -- example.ucl
UCL: true
```

Use `cargo run -- --help` to display command-line help, or start an
interactive session by running `ucl` without arguments:

```console
$ cargo run
UCL 1.3.0 interactive mode — type :help for help.
>>> let x = 40;
>>> x + 2;
42
>>> len("hé");
2
>>> str(x + 2) + "!";
42!
>>> use "math.ucl" as math;
>>> math.double(21);
42
>>> fn make(base) { return fn(n) { base + n; }; };
>>> let add5 = make(5);
>>> add5(37);
42
>>> :quit
```

Bindings persist across lines, definitions may span multiple lines (the
`... ` continuation prompt appears while an entry is incomplete), and errors
do not end the session.

## Requirements

- Rust stable with Cargo (see `rust-toolchain.toml`) — only needed when
  building from source; prebuilt binaries are available

## Install

Prebuilt binaries for Linux x86_64, macOS (Apple Silicon and Intel), and
Windows x86_64 are attached to each
[release](https://github.com/letridung07home/Universal-Coding-Language/releases/latest).

**Linux:**

```sh
curl -LO https://github.com/letridung07home/Universal-Coding-Language/releases/latest/download/ucl-x86_64-linux.tar.gz
tar -xzf ucl-x86_64-linux.tar.gz
sudo mv ucl-x86_64-linux/ucl /usr/local/bin/
```

**macOS** — pick the archive matching your architecture (`aarch64` for Apple
Silicon, `x86_64` for Intel):

```sh
curl -LO https://github.com/letridung07home/Universal-Coding-Language/releases/latest/download/ucl-aarch64-macos.tar.gz
tar -xzf ucl-aarch64-macos.tar.gz
sudo mkdir -p /usr/local/bin && sudo mv ucl-aarch64-macos/ucl /usr/local/bin/
```

**Windows** — download `ucl-x86_64-windows.zip` from the latest release,
extract it, and place `ucl-x86_64-windows\ucl.exe` somewhere on your `PATH`
(a `.tar.gz` is also provided if you prefer).

Verify any download against the `sha256sums.txt` file attached to the same
release.

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
│   ├── guarantees.md    # Compatibility guarantees
│   ├── roadmap.md       # Milestones and project direction
│   └── spec.md          # Language syntax and semantics
├── src/
│   ├── main.rs          # `ucl` CLI entry point
│   ├── lib.rs           # Public Rust API
│   ├── source.rs        # Source files, byte positions, and spans
│   ├── diagnostic.rs    # Structured diagnostics
│   ├── lexer.rs         # Source text to tokens
│   ├── parser.rs        # Tokens to AST
│   ├── evaluator.rs     # AST evaluation
│   ├── module.rs        # `use` statement loading and import machinery
│   ├── render.rs        # Diagnostic rendering with source excerpts
│   └── repl.rs          # Interactive session loop
├── tests/
│   ├── cli.rs           # End-to-end CLI and REPL tests
│   ├── property.rs      # Property-based tests
│   └── smoke.rs         # Public API smoke tests
└── .github/workflows/   # CI, release, and fuzz workflows
```

## Roadmap

Version 1.0 marked the first stable release; the guarantees above now apply in
full. UCL 1.1 adds the first built-in, `len(string)`, UCL 1.2 adds read-only
namespaced module imports, and UCL 1.3 rounds out the built-in prelude with
`str`, `type`, `upper`, `lower`, and `contains`. UCL 1.4 broadens the
prebuilt release binaries to macOS and Windows. Future directions — a richer
package story — are sketched in the [project roadmap](docs/roadmap.md).

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
