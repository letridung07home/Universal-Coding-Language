# UCL compatibility guarantees

This document describes what the UCL project promises to keep stable across
releases, and what may change. It applies to the language, the `ucl` command
line interface, and the Rust library crate. These guarantees apply in full as
of version 1.2.0.

## Versioning policy

UCL uses semantic versioning. Breaking changes ship only in major versions;
every breaking change is listed under a **Breaking** heading in the changelog
entry for the release that introduces it.

The crate declares a minimum supported Rust version (`rust-version` in
`Cargo.toml`, currently 1.85, the first release supporting edition 2024).
The MSRV may rise in minor releases but never within a patch release, and
an MSRV increase is noted in the changelog entry that introduces it.

## Language compatibility

The language specification (`docs/spec.md`) defines the language. What is
guaranteed:

- Programs that evaluate successfully in one release continue to evaluate
  with the same result in later releases, except where a changelog entry
  declares a breaking language change.
- The set of reserved keywords only grows; a keyword can never become an
  identifier again without a major-version breaking change.

What is *not* guaranteed between releases:

- New syntax or new value forms may appear in any release.
- Programs that rely on unspecified behavior (behavior not defined in
  `docs/spec.md`) have no compatibility protection.
- Completion of programs whose resource consumption is unbounded or exceeds a
  documented evaluator safeguard is not guaranteed. Evaluator resource limits
  may be introduced or tightened in a minor release to prevent host exhaustion;
  their current values and resulting error categories are specified in
  `docs/spec.md`.
- A pipeline run retains at most 1,000 diagnostics. Once that cap is reached,
  evaluation stops; the retained diagnostics remain available through the
  library API and CLI rendering.

## Diagnostics

Diagnostics are messages with a severity and an optional source span,
rendered to stderr with a source excerpt. Guaranteed:

- A program whose evaluation fails produces at least one diagnostic with
  error severity on stderr, and no value on stdout.
- Diagnostics never appear on stdout.
- The *categories* of errors documented in the specification (§7) are part
  of language compatibility: if something is an error in one patch release
  it does not silently become valid code.

Explicitly *not* guaranteed:

- Exact diagnostic wording, severity choices, span extents, and excerpt
  layout may change in any release. Tools should not parse diagnostic text;
  they should use the library API instead.

## Spans and source positions

- Byte offsets index into UTF-8 source text; spans always fall on character
  boundaries when produced by the pipeline.
- Diagnostic rendering counts columns in characters, not bytes.
- Invalid public spans passed through the library API are rejected rather
  than panicking.

## Library API

The crate's public items — `Lexer`, `Parser`, `Evaluator`,
`Environment::evaluate_in` plumbing, AST types, `Value`, diagnostics, and
source types — follow the same versioning policy as the language. The standard
prelude is available in every newly created `Environment`, including one
created after a REPL reset; user bindings may shadow its names:

- Additive changes (new variants, new methods, new fields on structs that
  are not exhaustively matched by design) may appear in any minor release.
- Removing or changing existing public signatures requires a declared
  breaking change.
- Enum variants added to `AstKind`, `Value`, `BuiltinFunction`, or `Keyword`
  make existing exhaustive matches non-exhaustive from the next minor release
  onward; downstream matchers must handle unknown cases or pin their dependency.
  Version 1.2 adds `AstKind::Member`, `Value::Module`, `ModuleValue`, and the
  contextual `TokenKind::ImportAs` token for aliased imports.
- Deterministic resource limits are part of the language contract: the
  8 MiB string-value limit, the 100,000-iteration loop cap, and — added in
  UCL 1.15 — the 256 MiB cumulative allocation budget over value data built
  during one evaluation, extended in UCL 1.16 to cover derived strings
  (`upper`, `lower`, `trim`, `slice`, `replace`) and list concatenation.
  Programs doing very large repeated transformations can now hit the
  budget's deterministic error where they previously ran to completion;
  accumulation through assignment remains linear and unaffected. Programs exceeding a limit fail with the
  corresponding error; the constants may change only in declared releases.
- One declared exception: UCL 1.13 changes `Value::List`'s payload from
  `Vec<Value>` to `Rc<Vec<Value>>`. The variant shipped in 1.11 with no known
  downstream consumers, so the payload change is announced here rather than
  through a major release; code constructing or destructuring `Value::List`
  must wrap or unwrap the `Rc` from 1.13 onward.

## Command line interface

Stable across releases:

- Exit codes: `0` successful evaluation (including empty/unit results),
  `1` a program failed to evaluate, `2` usage errors such as unknown
  options or unreadable files.
- Program output goes to stdout; all diagnostics and usage messages go to
  stderr.
- Running `ucl` with no arguments starts the interactive REPL; `--help`
  and `--version` print help and version information respectively.

### Interactive sessions

The REPL's user-facing behavior is stable in practice and documented here,
but it is informational rather than a formal compatibility surface:

- The primary prompt is `>>> `; a continuation prompt `... ` appears while
  an entry is incomplete.
- The banner names the version on startup.
- Meta commands: `:help` prints available commands, `:reset` discards all
  bindings, `:quit` (or `:exit`) ends the session, and end-of-input exits.
- Each entry evaluates against a persistent environment; errors do not
  end the session.

## Source formatter

The `ucl fmt` subcommand, introduced in 1.14, is stable across releases:

- Layout rules are deterministic: four-space indentation, one statement per
  line with a terminating semicolon (including after braced statements),
  single spaces around binary operators, and blocks expanded across lines.
- Comments are preserved. A comment sharing a source line with preceding
  code stays trailing on that output line; other comments become standalone
  lines at the enclosing indentation. Comment text is never altered.
- Formatting is idempotent: formatting formatted output reproduces it byte
  for byte.
- Formatting preserves evaluation: a program that lexes, parses, and
  evaluates cleanly evaluates to the same value after formatting.
- Output always ends with exactly one newline; CRLF input is normalized to
  LF.
- Exit codes follow the CLI convention: `0` when the output is formatted,
  `1` when `--check` finds unformatted input or the source has errors, `2`
  for usage and I/O problems. Files with errors are never rewritten.
- Future formatter releases may refine layout details only in ways that
  keep these properties; any exception would be a declared breaking change.

## Module loading

- Module paths resolve relative to the importing file; this resolution rule
  is normative (see the specification §5.5).
- The evaluator reads module files from the local filesystem at evaluation
  time. Sandbox restrictions, network fetching, and package registries do
  not exist today and would be additive features.
- Both `use "path.ucl";` flat imports and `use "path.ucl" as alias;`
  namespaced imports are stable language forms. Aliased imports expose a
  read-only module namespace, and a completed module is evaluated at most once
  per canonical path per session.
