# Changelog

All notable changes to UCL are documented here.

## Unreleased

### Fixed

- The evaluator now bounds total AST work across an evaluation, preventing
  independently capped nested loops from timing out the fuzz pipeline.
- List allocation accounting charges the actual `Value` slot size, preventing
  nested list-growth loops from exhausting host memory before the cumulative
  allocation budget trips. The fuzz timeout and OOM inputs are retained as
  pipeline regression seeds.

## 1.16.1 - 2026-08-24

### Internal

- Decomposed the evaluator's monolithic statement dispatcher (~710 lines,
  one `match` arm per construct) into per-construct methods (`eval_while`,
  `eval_for`, `eval_assignment`, `eval_call`, `eval_function_declaration`,
  `eval_if`, `eval_index`, `eval_member`) and grouped the built-in
  dispatcher into conversion, string-transformation, and collection
  helpers with an exhaustive category router. No behavior change:
  diagnostics, semantics, resource limits, and performance are identical;
  the full suite, formatter property tests, and diagnostic spot-checks
  verify it.

## 1.16.0 - 2026-08-24

### Fixed

- The nightly fuzz run timed out again despite the 1.15 allocation budget:
  several value-copying operations were still uncharged, so loops could
  rebuild large values hundreds of thousands of times while the cumulative
  budget barely moved. The budget now charges every operation whose cost
  scales with value size: `upper` and `lower` charge the bytes they copy,
  `trim` and `slice` (string and list forms) charge their results, and list
  concatenation charges both operands instead of only the appended side.
  The runaway input family now trips the budget within milliseconds.

### Added

- An in-place fast path for `items = items + [x]` assignment, mirroring the
  string and `append` fast paths: list accumulation through `+` stays
  linear in total work even though general concatenation is now charged
  honestly. Aliased lists still copy transparently; observable semantics
  are unchanged.
- The Fuzz workflow uploads libFuzzer artifacts on failure, so future
  timeout inputs survive the run and can be reproduced (the v1.15 timeout
  input was lost because nothing preserved it).

### Documented

- Pure read work (comparisons, membership tests) over very large values in
  long loops remains bounded by the loop cap times value size rather than
  the allocation budget; this is now stated explicitly in the
  specification.

## 1.15.0 - 2026-08-24

### Added

- A deterministic cumulative allocation budget (256 MiB) over the value data
  built during one evaluation. String operations charge the bytes they copy;
  list growth charges new elements. Exceeding the budget stops evaluation
  with `evaluation exceeded its total allocation budget of ... bytes`,
  joining the existing string-size, loop-count, call-depth, and nesting
  limits. The limit is documented in the specification and covered by the
  compatibility guarantees.

### Fixed

- The GitHub Actions fuzz run timed out on an input that loops forever while
  concatenating onto a growing string: legal operations, quadratic total
  work, tens of seconds under sanitizer instrumentation. The allocation
  budget now stops that entire input family within milliseconds.
- List accumulation through assignment (`items = append(items, x)`) is now
  linear in total work via an in-place fast path mirroring the existing
  string one; previously each append copied the whole list (100,000 appends:
  minutes before, ~80 ms now). Aliased lists still copy transparently, so
  observable semantics are unchanged.
- The fuzz workflow's per-run timeout rose from 25 to 60 seconds for margin
  against slow-but-bounded inputs.

## 1.14.0 - 2026-08-24

### Added

- `ucl fmt`, a deterministic source formatter, completing the roadmap:
  - `ucl fmt <file>` rewrites the file in place; `ucl fmt -` pipes stdin to
    stdout; `--check` exits 1 when a file needs formatting (CI gate).
  - Four-space indentation, one statement per line, canonical operator
    spacing, expanded blocks; multiline list literals stay expanded.
  - Comments are preserved: trailing comments stay on their line, others
    become standalone lines at the enclosing indentation; nothing is ever
    dropped.
  - Formatting is idempotent and preserves evaluation exactly (literals are
    reproduced verbatim from their source spans); property tests check both
    invariants over a corpus and thousands of random inputs.
  - The lexer exposes `tokenize_with_comments`, a secondary stream of
    comment spans; the plain token stream is unchanged.
  - Formatter behavior is covered by a new "Source formatter" section in
    the compatibility guarantees.

## 1.13.0 - 2026-08-23

### Changed

- Internal quality release; language behavior is unchanged.
- Lists now store their elements behind `Rc<Vec<Value>>`. Rebinding,
  capturing, or passing a list copies a reference instead of every element —
  rebinding a 16k-element list in a loop measured ~27× faster. This changes
  the public payload type of `Value::List`; see the declared exception in
  the compatibility guarantees.
- Functional list updates reuse their buffer through `Rc::make_mut` when
  the argument is uniquely owned. `append` remains O(n) per call when the
  list may be aliased — inherent to functional updates under dynamic
  aliasing — but constants improved ~2× in accumulation workloads.

### Fixed

- The AST mutation-classification match is now exhaustive: adding an
  `AstKind` variant without deciding whether it can execute code is a
  compile error rather than a silent default. The rewrite itself caught one
  unclassified case (member access on arbitrary expressions).

## 1.12.1 - 2026-08-23

### Fixed

- Side effects no longer execute twice when an append-style assignment such
  as `s = s + <block>` falls back from the in-place fast path. Blocks
  containing `for` loops or `let` declarations were misclassified as
  mutation-free, so a non-string result triggered re-evaluation — running
  the block's loop or initializer twice before the (unavoidable) type error.

## 1.12.0 - 2026-08-23

### Added

- List toolkit round 2, completing what 1.9 did for strings:
  - `append(list, item)` returns a new list with `item` added at the end.
    It is functional: the original list is untouched, so lists can now be
    accumulated inside loops (`items = append(items, x);`).
  - `+` concatenates two lists into a new list, mirroring string
    concatenation; mixing types remains an error.
  - `slice(list, start, end)` extracts a sub-list with the same strict
    bounds checking as string slicing.
  - `find(list, item)` returns the index of the first element equal to
    `item` (using `==`, so nested lists compare element by element), or
    `-1` when absent.

## 1.11.0 - 2026-08-23

### Added

- Immutable list values, the first aggregate type:
  - List literals `[1, "two", [3]]` nest arbitrarily; `[]` is the empty
    list.
  - Indexing with `items[0]` yields elements; `"hello"[1]` indexes strings
    the same way, yielding one-character strings. Indices are zero-based
    and strict: negative or out-of-range indices are runtime errors.
  - `==`/`!=` compare lists element by element, recursing through nesting.
  - `for x in list { ... }` iterates elements in order.
  - `len` counts a list's elements and `contains` tests list membership
    using the same equality as `==`.
- Lists echo their full contents — `[1, "two", true]` — with element
  strings quoted; this text is shared by the CLI echo, the REPL, and
  `str()`.

Lists have no mutation yet: element assignment and concatenation remain
undefined and can be added later without breaking changes.

## 1.10.0 - 2026-08-23

### Added

- The `for` loop, the first new statement form since modules:
  - Range form `for i in start..end { ... }` iterates over a half-open
    numeric range, consistent with `slice` indexing; bounds are evaluated
    once before the first iteration.
  - String form `for ch in value { ... }` iterates over the Unicode scalar
    values of a string, binding each as a one-character string.
  - Empty and inverted ranges perform zero iterations; iterating any other
    type is a runtime error. Each iteration binds a fresh body-scoped
    variable. `break` and `continue` apply, and the 100,000-iteration cap
    guards runaway ranges.
- Two new reserved words come with the syntax: `for` and `in` can no longer
  be used as identifiers (matching the `break`/`continue` precedent).

## 1.9.0 - 2026-08-23

### Added

- Four string built-ins, completing the string toolkit:
  - `find(haystack, needle)` returns the scalar-value index of the first
    occurrence of `needle`, or `-1` when it does not occur.
  - `replace(source, pattern, replacement)` returns a copy of `source` with
    every occurrence of `pattern` replaced; an empty pattern is a runtime
    error.
  - `trim(value)` strips leading and trailing whitespace.
  - `slice(value, start, end)` extracts the substring between scalar-value
    indices with strict bounds checking: negative, out-of-range, or inverted
    indices are runtime errors.

## 1.8.0 - 2026-08-23

### Added

- The `-e`/`--eval <code>` flag evaluates an inline program without a source
  file, reporting runtime errors against an `<eval>` source excerpt.
- A file name of `-` reads the program from standard input, enabling shell
  pipelines such as `echo '1 + 1;' | ucl -`; errors report against
  `<stdin>`.
- The `int(value)` built-in: strings with an optional sign and ASCII decimal
  digits parse to integers, integers pass through unchanged (mirroring
  `str`), and parse failures, out-of-range values, and type errors are
  reported like the other built-ins.

### Changed

- The evaluator's module docs no longer describe operator semantics as
  pending; they now point at the normative specification.

## 1.7.0 - 2026-08-23

### Changed

- Internal quality release with no language, CLI, or public API changes:
  - `src/evaluator.rs` is split into a module directory — `value.rs`
    (runtime values), `environment.rs` (scopes and binding resolution),
    `builtins.rs` (the prelude enum), and `tests.rs` (the unit suite) —
    with `mod.rs` re-exporting the same public names as before.
  - Built-in names now have a single source of truth in
    `BuiltinFunction::name`; arity-check call sites no longer repeat the
    string literals.
  - CI gains a dependency-audit job running `cargo deny` (advisories,
    licenses, duplicate crates) configured by a new `deny.toml`.

## 1.6.0 - 2026-08-23

### Added

- The `break` and `continue` statements for `while` loops. `break` exits the
  innermost enclosing loop; `continue` skips to its next condition check.
  Both unwind through nested blocks and conditionals, evaluate to unit, and
  are consumed by their matching loop.
- A `break` or `continue` that reaches a function-call boundary or the
  program/module top level without a matching loop is a runtime error
  (`` `break` outside of a loop `` / `` `continue` outside of a loop ``), so a
  function called from inside a loop cannot break its caller's iteration.
- `break` and `continue` are now reserved keywords and can no longer be used
  as identifiers; the compatibility guarantees explicitly allow the reserved
  set to grow in minor versions.
- Evaluator, CLI, property-pipeline, and fuzz-corpus coverage for loop
  control.

## 1.5.0 - 2026-08-23

### Added

- Extensionless imports: `use "math";` and `use "math" as math;` resolve
  `math.ucl` when no file named exactly `math` exists. Imports written with
  an explicit `.ucl` extension behave exactly as before.
- Module search directories, consulted when an import cannot be found next
  to the importing file:
  - the `ucl` binary accepts repeatable `-p/--path <dir>` options;
  - `UCL_PATH` directories (separated by the platform path list separator)
    are consulted after any `-p/--path` flags; and
  - library consumers configure paths through the new public
    `Environment::add_search_path`. The REPL applies both sources and keeps
    them across `:reset`.
- Not-found import errors now list every candidate location that was tried.
- The public API change is additive: default resolution is unchanged when no
  search paths are configured.

### Fixed

- Import resolution now reports missing modules at resolution time instead of
  attempting a filesystem read of a nonexistent candidate path.

## 1.4.0 - 2026-08-23

### Added

- Prebuilt macOS release artifacts for Apple Silicon (`aarch64-macos`) and
  Intel (`x86_64-macos`), and Windows x86_64 artifacts in both `.zip` and
  `.tar.gz` form, alongside the existing Linux x86_64 archive. Every artifact
  is covered by a combined `sha256sums.txt`.
- CI now runs the test suite on macOS and Windows in addition to Linux, so
  platform-specific breakage surfaces before a release is tagged.

### Changed

- The release workflow builds each platform natively in a matrix job and
  publishes all artifacts from a single follow-up job; release notes and
  checksum handling are unchanged.

## 1.3.0 - 2026-08-23

### Added

- Five new built-in functions, joining `len` in the prelude:
  - `str(value)` returns the same text the CLI and REPL echo for the value:
    integers and booleans render as written, strings are unchanged, functions
    render as `<function>`, modules as `<module>`, and unit as `unit`.
  - `type(value)` returns `"integer"`, `"boolean"`, `"string"`, `"function"`,
    or `"module"`.
  - `upper(string)` and `lower(string)` perform Unicode case conversion.
  - `contains(haystack, needle)` reports whether one string contains another.
- The public `Value::display_text` method, the shared implementation behind
  both result echoing and the `str` built-in, so the two cannot disagree.
- Strings produced by built-ins honor the deterministic 8 MiB value limit.
- Evaluator, CLI, REPL-reset, property, and fuzz-corpus coverage for the new
  built-ins.

### Changed

- Built-in arity errors now use a uniform message form:
  `` `name` expected N argument(s), received M ``.

## 1.2.0 - 2026-08-22

### Added

- Aliased local-file imports: `use "path.ucl" as module;` binds one read-only
  module namespace, and `module.name` resolves an exported value or callable.
  Existing `use "path.ucl";` flat imports remain supported unchanged.
- Completed module exports are cached per canonical path for each session, so
  aliases, flat imports, and mixed import forms reuse one module evaluation.
- The public `ModuleValue` type, `Value::Module` variant,
  `AstKind::Member` variant, and contextual `TokenKind::ImportAs` token.
- Parser, evaluator, CLI, REPL, and fuzz-corpus regression coverage for
  aliased imports, member access, member errors, import ordering, and cache
  reuse.

### Changed

- Flat import collisions are now checked before any export is copied, so a
  failed legacy import cannot leave a partial set of module bindings behind.
- Evaluating a namespace as a final CLI or REPL expression renders as
  `<module>`.

## 1.1.0 - 2026-08-22

### Added

- The first built-in function: `len(string)`, which returns the number of
  Unicode scalar values in its string argument. Built-ins live in a prelude
  available to every new environment, including a reset REPL session; ordinary
  user bindings may shadow their names.
- The public `BuiltinFunction` type and `Value::Builtin` variant, allowing
  library consumers to recognize prelude callables.
- Regression coverage for `len` across evaluator, CLI, REPL-reset, and fuzz
  pipeline entry points.

### Changed

- String values are now bounded to 8 MiB of UTF-8 bytes. Decoded literals or
  concatenations that would exceed the limit report a runtime error and stop
  the current evaluation, preventing repeated concatenation from exhausting
  host memory.
- The pipeline fuzz corpus includes a bounded exponential-concatenation seed
  that exercises the string-value safeguard. The harness now mirrors the
  production pipeline by skipping evaluation after lexer or parser errors.
- Diagnostic retention is capped at 1,000 entries per pipeline run, and
  evaluation stops at that cap so repeated runtime failures cannot exhaust host
  memory.

## 1.0.0 - 2026-08-22

The first stable release. The language, command-line interface, Rust library
API, and error categories described in `docs/guarantees.md` are now covered by
compatibility guarantees in full: breaking changes ship only in major
versions.

No code changes since 0.8.2; this release converts documentation caveats into
promises.

### Changed

- The compatibility guarantees now apply without the pre-1.0 escape hatch:
  programs that run in this release keep running, and error categories are a
  stable interface.
- Exact diagnostic wording remains deliberately non-contractual; tools should
  use the library API.
- The REPL's user-facing behavior (prompts, banner, meta commands) is
  documented as stable de-facto behavior.
- README, specification status, roadmap, and development guide updated to
  reflect the stable release.

## 0.8.2 - 2026-08-22

### Changed

- Internal refactor: all module-loading logic (`use` statements, path
  resolution, cycle detection, import merging) moved from `evaluator.rs`
  into a dedicated `module.rs`. No public API or behavior change.
- Fixed a brittle REPL test assertion that matched any `2` in the output
  (including the version banner) instead of checking that nothing runs
  after `:quit`.

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
