# Changelog

All notable changes to UCL are documented here.

## 0.8.1 - 2026-08-22

### Added

- The fuzz corpus seed files are now tracked in the repository, so a fresh
  clone fuzzes from meaningful input covering strings, control flow,
  closures, modules, and block comments.
- A CI check builds the documentation with warnings denied.
- A declared minimum supported Rust version: 1.85 (`rust-version` in
  `Cargo.toml`).

### Changed

- Fixed two rustdoc warnings (a broken intra-doc link and a redundant
  explicit link target).
- Deduplicated the README feature list.

## 0.8.0 - 2026-08-22

### Added

- Block comments: `/* ... */` may span multiple lines and nest; an
  unterminated block comment is an error.
- Relational operators `<`, `>`, `<=`, `>=` now accept two strings,
  comparing lexicographically by Unicode scalar value. Mixing a string with
  an integer remains an error.
- Compatibility guarantees document (`docs/guarantees.md`) covering
  language, diagnostics, spans, library API, CLI, and module-loading
  stability expectations.

### Changed

- The specification no longer lists block comments as future work.

## 0.7.0 - 2026-08-22

### Added

- File-based modules: `use "path.ucl";` imports another source file. Paths
  resolve relative to the importing file's directory (working directory in
  interactive sessions). A module is evaluated once per session in an
  isolated global scope, and its top-level bindings — including names it
  imported itself — are copied into the importer's global scope. Name
  collisions abort the import with an error.
- Circular imports are detected and reported instead of looping; unreadable
  files and errors inside a module are reported anchored at the `use` site.
- **Breaking:** `use` is now a reserved keyword and can no longer be used as
  an identifier. Imports are only valid at the top level of a program.
- The evaluator now reads module files from the filesystem when evaluating a
  `use` statement.

## 0.6.0 - 2026-08-22

### Added

- Interactive REPL: running `ucl` without a file starts a session
  (`>>> ` prompt) where bindings persist across inputs. Definitions that end
  in the middle of a construct prompt a continuation line (`... `) instead of
  reporting an error; runtime and syntax errors do not end the session. Meta
  commands: `:help`, `:reset`, `:quit` (Ctrl-D also exits).
- `Environment` is now public with `Evaluator::evaluate_in`, allowing
  library users to evaluate many programs against one persistent set of
  bindings.
- `Parser::is_incomplete` reports whether the last parse stopped because the
  input ended mid-construct rather than because of genuinely malformed
  syntax.

### Changed

- Function values now carry their defining source, so a function created in
  one program can be called from another (essential for multi-line REPL
  sessions). **Breaking (library API):** `SourceFile` gained `Clone`/`Debug`
  derives, and `FunctionValue` gained a private source field.

### Removed

- Running `ucl` with no arguments no longer prints a usage error; it starts
  the interactive REPL instead.

## 0.5.0 - 2026-08-22

### Added

- Anonymous function literals: `fn(x) { x * 2; }` is an expression producing a
  `function` value. Literals can be stored, passed as arguments, returned from
  functions, and called immediately (`fn(a) { a; }(1)`).
- Nested named function declarations: the v0.4 restriction to program scope is
  lifted; declarations now bind in any scope.
- Explicit `return` statements: `return expression;` exits the innermost call
  immediately (unwinding through blocks and loops); bare `return;` returns
  unit. A `return` executed outside any function call is an error.
- Closures with capture-by-value semantics: when a function value is created
  it snapshots every non-global binding visible at that point. Globals are
  excluded and always resolve dynamically, preserving recursion of top-level
  functions. Known limitation: a literal cannot call itself through the
  variable being defined (`let f = fn(n) { f(n); }`).
- **Breaking (library API):** `AstKind::Function.name` is now an
  `Option<Span>`, and new variants `AstKind::Return` were added;
  `FunctionValue` gained a private capture map.

### Changed

- The maximum active function-call depth was raised from 64 to 128.

## 0.4.0 - 2026-08-22

### Added

- Named function declarations: `fn name(parameter, ...) { ... }`.
- Positional function calls with left-to-right argument evaluation and exact
  arity checking.
- Implicit function return values: a function evaluates to the value of its
  final statement, or `unit` when its body is empty.
- Recursive global functions. Function bodies resolve their own parameters and
  global bindings, never a caller's local bindings.
- Diagnostics for calls to non-function values, incorrect argument counts,
  duplicate parameters, and function declarations outside program scope.
- A 64-active-call safety limit that reports an error instead of overflowing the
  host stack during unbounded recursion.

### Changed

- **Breaking (library API):** `Value` has a new `Value::Function(FunctionValue)`
  variant, and `AstKind` has new `Function` and `Call` variants.
- UCL's dynamic runtime type policy is now explicitly documented; static type
  checking, function literals, nested functions, and closures remain future
  work.

## 0.3.0 - 2026-08-22

### Added

- String literals (`"hello"`) with the escape sequences `\n`, `\t`, `\\`,
  and `\"`. Unterminated literals, raw newlines inside strings, and unknown
  escape sequences are errors.
- String concatenation with `+` and equality comparison (`==`, `!=`) of two
  strings. Mixing strings with other operand types is an error, consistent
  with existing operator strictness.
- Conditional expressions: `if condition { ... } else { ... }`. Parentheses
  around the condition are optional; `else if` chains are supported. The
  expression evaluates to the value of the taken branch (or `unit` when the
  condition is false and no `else` is present). Only the taken branch runs.
- `while` loops as statements, evaluating to `unit`. The body runs in its own
  lexical scope.
- A loop-iteration cap (100,000 iterations per `while` loop): a condition
  that never becomes false aborts the loop with an error instead of hanging
  the interpreter.
- New reserved keywords: `if`, `else`, `while`.
- **Breaking (library API):** new `Value::Str(String)` variant and new AST
  kinds `AstKind::StringLiteral`, `AstKind::If`, and `AstKind::While`.

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
