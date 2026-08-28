# UCL roadmap

This roadmap prioritizes correctness and a stable language foundation before
major features. It describes direction rather than replacing GitHub Issues;
concrete implementation work should be tracked in issues as it is scheduled.

## Near term: correctness and resilience

- [x] Recognize `let` as an actual keyword instead of accepting any identifier
      in the declaration position
- [x] Stop evaluation when lexing or parsing reports errors
- [x] Make evaluator failures explicit rather than representing them as `unit`
- [x] Validate source spans and prevent invalid public spans from panicking
- [x] Add regression tests for overflow, malformed syntax, UTF-8 diagnostics,
      nested scopes, and CLI argument handling
- [x] Add parser and evaluator nesting limits

## Language foundation

- [x] Define keyword, assignment, operator, and runtime-error semantics
- [x] Decide that UCL is dynamically typed for the current interpreter stage
- [x] Add boolean literals and equality operators
- [x] Add strings and string operations
- [x] Add conditional and looping constructs
- [x] Add named functions, parameters, calls, implicit return values, and recursion
- [x] Add function literals, nested functions, explicit `return` statements, and closures

## Tooling and ecosystem

- [x] Add property tests and fuzz targets for the lexer and parser
- [x] Publish diagnostic and compatibility guarantees
- [x] Design a module and package system
- [x] Add an interactive REPL
- [x] Provide release artifacts and installation instructions

## Completed foundation

- [x] Write the initial language specification
- [x] Implement the lexer
- [x] Implement the parser and abstract syntax tree
- [x] Implement the evaluator
- [x] Add the `ucl` command-line interface
- [x] Render diagnostics with source excerpts
- [x] Add integration tests and CI checks

## After 1.0

Version 1.0 marks the first stable release: the language, CLI, library API,
and error categories are covered by the
[compatibility guarantees](guarantees.md). Future directions, sketched rather
than scheduled:

- [x] Add the first built-in function: `len(string)` for Unicode scalar-value length
- [x] Add additional built-in functions: `str`, `type`, `upper`, `lower`, and
      `contains`
- [x] Add namespaced imports (`use "path" as module;`) and member access
      (`module.name`) alongside legacy flat imports
- [x] Broader platform coverage for release artifacts (macOS and Windows
      joined Linux, with CI coverage for each supported OS)
- [x] A richer module story beyond local files: extensionless imports and
      configurable search directories (`-p/--path`, `UCL_PATH`)

Every planned v1 item on this roadmap is complete as of UCL 1.19.0, the final
stable v1 release. The language, CLI, library API, and error categories remain
covered by the [compatibility guarantees](guarantees.md); the next direction is
the draft, explicitly breaking [v2.0.0 static-type-checking goal](v2-goal.md).

## Beyond 1.7

Sketched rather than scheduled:

- [x] More string manipulation built-ins (`find`, `replace`, `trim`, slicing)
      (UCL 1.9)
- [x] A `for` loop iterating over strings or numeric ranges (UCL 1.10)
- [x] Aggregate values such as lists (UCL 1.11 adds immutable lists with
      indexing, equality, and iteration; UCL 1.12 adds functional `append`,
      concatenation, and list support in `slice` and `find`)
- [x] A source formatter (`ucl fmt`) with deterministic layout, comment
      preservation, idempotent output, and a CI-friendly `--check` mode;
      this completes the roadmap (UCL 1.14)
- [x] Scripting ergonomics: inline programs with `-e/--eval`, piped input
      through `-`, and the `int()` conversion built-in (UCL 1.8)

Each of these is additive; none would require a breaking change. UCL 1.1 adds
a deterministic 8 MiB string-value limit to keep repeated concatenation from
exhausting host memory. UCL 1.2 adds read-only namespace aliases backed by a
per-session completed-export cache, keeping existing flat imports intact.
UCL 1.3 expands the built-in prelude with value conversion, type inspection,
and string helpers, all sharing the result-echo rendering through one
implementation. UCL 1.4 broadens release artifacts to macOS (Apple Silicon
and Intel) and Windows x86_64, with CI running the test suite on each
supported platform. UCL 1.5 completes the roadmap with extensionless import
paths and configurable module search directories, keeping imports next to
the importing file as the highest-priority resolution.

## Internal quality backlog

Refactor work tracked for future releases; none changes language behavior:

- [x] Replace wildcard match fallbacks over `AstKind` (such as
      `may_mutate_bindings`'s `_ => false`) with explicit handling so adding
      a variant cannot silently skip a check again — the 1.12.1
      double-evaluation fix was the cautionary tale, and the exhaustive
      rewrite immediately caught an unclassified `Member` arm (UCL 1.13)
- [x] Move list storage behind shared references (`Rc<Vec<Value>>`) so
      rebinding, capturing, and passing lists copy a reference instead of
      every element (~27× faster list rebinding in the 1.13 gate). Note:
      `append` through assignment became linear in total work in UCL 1.15
      via an in-place fast path; a functional update still copies when the
      list may be aliased, which is inherent to dynamic aliasing (UCL 1.13,
      refined in 1.15)
- [x] Charge every value-copying operation against the cumulative
      allocation budget — derived-string built-ins and list concatenation
      included — with in-place fast paths keeping accumulation idioms
      linear, and preserve fuzz artifacts when a nightly run fails
      (UCL 1.16)
- [x] Decompose the evaluator's monolithic statement dispatcher
      (~710 lines) into per-construct methods and group the built-in
      dispatcher by category, with no behavior change (UCL 1.16.1)
- [x] Performance optimization of the evaluator's per-construct dispatch
  methods (`eval_while`, `eval_for`, `eval_assignment`, etc.):
  reduced `pending_flow` overhead and streamlined resource-state
  checks through `should_stop_evaluation()` (UCL 1.17.0)
- [x] Read-only import-graph inspection that resolves the complete transitive
  `use` graph without evaluating source, mirrors module path precedence and
  extensionless lookup, and is available from both `ucl --list-imports` and
  the public `ucl::module` API (UCL 1.18.0)
- [x] Final v1 compatibility and documentation audit, backed by a release
  metadata check that confirms the manifest, lockfile, release notes, and
  stable-contract references remain aligned (UCL 1.19.0)

### Release pipeline status

- **UCL 1.19.0** is the final v1 release. Its tag-driven workflow builds Linux
  x86_64, macOS (Apple Silicon and Intel), and Windows x86_64 artifacts and
  publishes their combined `sha256sums.txt` manifest. The metadata gate runs
  before the release build to keep versioned documentation and Cargo metadata
  synchronized.
- The scheduled Fuzz workflow previously reported failures on 2026-08-26 and
  2026-08-27. UCL 1.17.2 added a cumulative loop-iteration budget to cover the
  reported nested-loop family, and the subsequent manually triggered fuzz run
  completed successfully. Scheduled fuzzing remains part of the release-health
  signal.

### Next direction: v2.0.0

Future work is no longer a blank page. `docs/v2-goal.md` defines the
major version goal: **optional static type annotations and compile-time
type checking** (`docs/v2-goal.md`). This replaces the pure dynamic-type
policy guaranteed since `v1.0` (`docs/spec.md` §3, `docs/guarantees.md`).
The v2 release requires:

- At least one declared `**Breaking**` heading in `CHANGELOG.md`.
- `docs/guarantees.md` rewritten with the new static-checking contract.
- `Cargo.toml` bumped to `2.0.0`.
- `docs/spec.md` updated with type syntax (`int`, `bool`, `string`, `list`,
  `function`, `unit`, `module`).

The sequential capstone plan (`1.18.0`, `1.19.0`) leading to `v2.0.0`
is documented in `docs/v1-release-plan.md`.
