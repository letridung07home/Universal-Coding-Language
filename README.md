# Universal Coding Language (UCL)

A universal coding language that excels in every task possible.

UCL is a small, expression-oriented language implemented in Rust. The project
provides an interpreter with a lexer, parser, evaluator, file-based modules,
an interactive REPL, a command-line interface, and source-aware diagnostics.

> [!NOTE]
> UCL 2.0.0 is the current stable release. It preserves UCL's dynamic default
> while adding optional static annotations and check-only or strict CLI modes.
> The deliberate v2 compatibility changes are documented in the
> [compatibility guarantees](docs/guarantees.md) and
> [v2 release record](docs/v2-goal.md).

## Features

- Signed 64-bit integers, booleans, and strings
- Immutable lists with literals, indexing, equality, `for` iteration,
  concatenation (`+`), and the `append`, `slice`, and `find` built-ins
- Checked arithmetic, comparison, and equality operators
- String concatenation with escape sequences and a deterministic 8 MiB value limit
- Built-in functions: `len(string)`, `str(value)`, `type(value)`,
  `upper(string)`, `lower(string)`, `contains(haystack, needle)`, `int(value)`,
  `find(haystack, needle)`, `replace(source, pattern, replacement)`,
  `trim(value)`, and `slice(value, start, end)`
- Short-circuiting logical operators
- `if`/`else` conditionals, `while` and `for` loops, and `break` and
  `continue` loop control
- Functions: declarations, literals, closures, and recursion
- Optional static type annotations on declarations and function signatures
- Static check-only (`ucl --type-check`) and strict evaluation (`ucl --strict-types`) modes
- `let` declarations and assignment
- Blocks with lexical scoping
- File-based modules: `use "path";` and `use "path" as math;`, with
  extensionless import paths, `-p/--path` search directories, and the
  `UCL_PATH` environment variable
- Read-only import-graph inspection through `ucl --list-imports`, with the
  same path-resolution behavior as evaluated modules and no program execution
- Line comments (`//`) and nesting block comments (`/* */`)
- String comparisons (`<`, `<=`, `>`, `>=`)
- Error diagnostics with source excerpts
- A deterministic source formatter (`ucl fmt`) that preserves comments and is
  idempotent
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

Format a file in place (or pipe through stdin) with the formatter:

```console
$ ucl fmt example.ucl
$ cat messy.ucl | ucl fmt -
$ ucl fmt --check src.ucl   # exits 1 when src.ucl needs formatting
```

Inspect resolved module dependencies without evaluating the program:

```console
$ ucl --list-imports -p ./lib app.ucl
/path/to/app.ucl
/path/to/app.ucl -> /path/to/lib/math.ucl
/path/to/lib/math.ucl -> /path/to/lib/numbers.ucl
```

Opt into static checking without executing source, or require fully typed
function signatures before normal evaluation:

```console
$ ucl --type-check app.ucl
$ ucl --strict-types app.ucl
```

```ucl
fn twice(value: int): int { value + value; };
let answer: int = twice(21);
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
>>> use "math" as math;
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
│   ├── fmt.rs           # Source formatter
│   ├── module.rs        # `use` statement loading and import machinery
│   ├── render.rs        # Diagnostic rendering with source excerpts
│   └── repl.rs          # Interactive session loop
├── tests/
│   ├── cli.rs           # End-to-end CLI and REPL tests
│   ├── property.rs      # Property-based tests
│   ├── formatting.rs    # Formatter idempotency and semantics tests
│   └── smoke.rs         # Public API smoke tests
└── .github/workflows/   # CI, release, and fuzz workflows
```

## Roadmap

Version 1.0 marked the first stable release; the guarantees above now apply in
full. UCL 1.1 adds the first built-in, `len(string)`, UCL 1.2 adds read-only
namespaced module imports, and UCL 1.3 rounds out the built-in prelude with
`str`, `type`, `upper`, `lower`, and `contains`. UCL 1.4 broadens the
prebuilt release binaries to macOS and Windows, UCL 1.5 completes the roadmap
with extensionless imports and configurable module search paths, UCL 1.6
adds `break` and `continue` loop control statements, UCL 1.7 is an
internal-quality release that reorganizes the evaluator's source layout and
adds a CI dependency audit, UCL 1.8 makes shell scripting easier with
inline `-e/--eval` programs, piped input through `-`, and the `int()`
conversion built-in, UCL 1.9 completes the string toolkit with the
`find`, `replace`, `trim`, and `slice` built-ins, UCL 1.10 adds `for`
loops over numeric ranges (`for i in 0..5`) and strings, UCL 1.11 adds
immutable list values with literals, strict bounds-checked indexing, deep
equality, and `for` iteration, UCL 1.12 completes the list toolkit
with functional `append`, list concatenation through `+`, and list support
in the `slice` and `find` built-ins, and UCL 1.13 is an internal-quality
release that makes AST classification compiler-enforced and stores lists
behind shared references. UCL 1.14 completes the roadmap with a deterministic
source formatter: `ucl fmt` rewrites files in place, pipes stdin to stdout,
supports CI checks with `--check`, and preserves comments. UCL 1.15 adds a
deterministic cumulative allocation budget that stops pathological accumulation
programs quickly, makes list accumulation through assignment linear in total
work, and raises the fuzz workflow's per-run timeout to 60 seconds; UCL 1.16
extends that budget to cover every value-copying built-in and list
concatenation, and preserves fuzz artifacts when a nightly run fails; UCL
1.17 adds a cumulative loop-iteration budget; UCL 1.18 adds a read-only
resolved import graph through `ucl --list-imports`; and UCL 1.19 completes the
stable v1 line with a compatibility audit and automated release-metadata gate.
UCL 2.0.0 completes the major-version transition to optional static checking:
typed declarations and function signatures are checked before evaluation,
`--type-check` performs analysis only, and `--strict-types` requires complete
function signatures. Future work is tracked in the [project roadmap](docs/roadmap.md).

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
