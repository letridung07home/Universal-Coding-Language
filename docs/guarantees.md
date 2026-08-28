# UCL v2 Compatibility Guarantees

**Active version:** `2.0.0`

This document describes the UCL project’s stability commitments from version
`2.0.0` onward. It covers the language, the `ucl` command-line program, and the
Rust library crate. The final v1 contract is preserved unchanged in
[`v1-guarantees.md`](v1-guarantees.md).

## Versioning policy

UCL uses semantic versioning. Breaking changes ship only in major versions, and
every breaking change appears under a **Breaking** heading in the corresponding
[`CHANGELOG.md`](../CHANGELOG.md) entry. Version `2.0.0` is the deliberately
breaking transition from the v1 dynamic-only type policy to optional static
annotations and checking.

The crate declares a minimum supported Rust version through `rust-version` in
`Cargo.toml`; it is currently Rust `1.85`. The MSRV may rise in a minor release,
but never in a patch release, and any increase is recorded in the changelog.

## Language compatibility

The language specification in [`spec.md`](spec.md) is normative. Within the
v2 line, source that **passes static checking** and evaluates successfully in
one release continues to evaluate with the same result in later releases unless
a later major release declares a breaking language change.

Unannotated programs retain UCL’s v1 dynamic semantics. Type annotations are
optional and activate checking for the annotated program unit; `--strict-types`
requires every function to have an annotated parameter-and-return signature.
Static checks reject known operator, declaration, assignment, return, function
argument, condition, indexing, and supported built-in mismatches before
execution. Incomplete information remains dynamic rather than being guessed.

The v2 type names `int`, `bool`, `string`, `list`, `function`, `unit`, and
`module` are contextual: they are recognized only immediately after `:`. They
remain usable as ordinary identifiers in all other positions. The v1 reserved
keyword set remains unchanged; future reserved-word additions require a major
release.

## Diagnostics and resource safeguards

Diagnostics have a severity and optional source span and are rendered on stderr
with a source excerpt. Static type failures are error-severity diagnostics and
prevent evaluation. A type-check-only invocation that succeeds produces no
stdout, and a failed check produces no evaluated value.

Exact wording, span extent, and excerpt layout are not stable parsing targets;
tools should consume the library types instead. The evaluator retains at most
1,000 diagnostics, its 8 MiB string-value limit, its 100,000-per-loop and
1,000,000-total-loop iteration limits, and its 256 MiB cumulative value-data
allocation budget.

Static checking has a separate deterministic budget of 1,000,000 visited AST
nodes per pipeline run. An annotated or strict input that exceeds it fails with
a type diagnostic before evaluation. This bound is independent of the runtime
limits and may change only in a declared release.

## Library API

The public crate surface includes `Lexer`, `Parser`, `Evaluator`, `Environment`,
AST types, `Value`, diagnostics, and source types. New v2 public typing APIs are
`Type`, `TypeContext`, `TypeName`, `TypeAnnotation`, and `Parameter`.

`Environment::new` now requires an explicit `TypeContext`, allowing embedded
applications to keep static binding state alongside a persistent runtime
environment. `Environment::default()` creates a fresh environment and empty
context when an application does not need to manage that state directly.
`Evaluator::type_check` performs checking without evaluation, while
`Evaluator::evaluate_typed` and `Evaluator::evaluate_typed_in` perform strict
checking before evaluation. The existing `evaluate` and `evaluate_in` methods
continue to evaluate unannotated source dynamically and automatically check
source containing annotations.

`AstKind::Let` now carries an optional annotation; `AstKind::Function` now
carries `Parameter` records and an optional return annotation. These structural
changes, along with the changed `Environment::new` signature, are intentional
v2 library API breaks. Public enum variants and fields may gain additive
capability within v2.x, but removal or incompatible signature changes require
v3.0.0.

## Command-line interface

The normal evaluation interface remains `ucl [-p <dir>]... [-e <code> | <file>]`.
The `-p/--path` and `UCL_PATH` module-search semantics, exit-code convention,
formatter, REPL, and read-only `--list-imports` command are preserved.

`ucl --type-check [--strict-types] <file>` parses and checks source without
executing it. `ucl --strict-types <file>` checks strictly and then evaluates
only when checking succeeds. The inspection-only `--list-imports` mode cannot
be combined with type-checking flags. Successful check-only commands exit `0`,
static or runtime program failures exit `1`, and usage or input errors exit `2`.

## Source formatter and modules

`ucl fmt` preserves annotations and emits canonical spacing such as
`let answer: int = 42;` and `fn twice(value: int): int { ... }`. It remains
idempotent and preserves evaluation for sources that lex, parse, type-check,
and evaluate successfully.

Module resolution remains relative-first with extensionless fallback and the
configured search directories. Imported source is statically checked before it
is evaluated whenever it contains annotations, so an annotated module cannot
introduce a deferred mismatch into an otherwise checked program.
