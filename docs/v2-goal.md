# UCL v2.0.0 Goal: Static Type Checking

**Target:** `2.0.0` (major version — requires declared breaking changes)  
**Status:** Draft goal — not yet scheduled  
**Branch:** `arena/01a03a0c-universal-coding-language`

---

## The goal

UCL `v2.0.0` introduces **optional static type annotations** and **compile-time type checking**, replacing the purely dynamic runtime type policy that has been guaranteed since `v1.0`.

Programs written without annotations continue to compile and evaluate with the same semantics as `v1.19.0`, but any program that relies on runtime-only type validation (e.g., passing an integer where a string is expected and catching it at evaluation time rather than at compile time) will now fail at parse or compile time when annotations are present. This is a deliberate break: the compatibility guarantee that *"programs that evaluate successfully in one release continue to evaluate with the same result in later releases"* is lifted in favor of a stronger contract: *"programs that type-check successfully evaluate with the same result; programs that fail type-checking are rejected before evaluation."*

---

## Why this requires v2.0.0

Per the final v1 `docs/guarantees.md` contract:

> *"UCL uses semantic versioning. Breaking changes ship only in major versions; every breaking change is listed under a **Breaking** heading in the changelog entry for the release that introduces it."*

No stable `v1.x` release has a `Breaking` heading (`CHANGELOG.md`). A `v2.0.0` must declare at least one. Static type checking creates multiple declared breaks:

### Declared breaks (`CHANGELOG.md` must list these)

1. **Language break — runtime type errors become compile-time errors for annotated code**  
   Previously guaranteed (`docs/spec.md` §3): *"UCL is dynamically typed. Values carry runtime types and operators validate their operands when evaluated; UCL performs no static type checking."*  
   `v2` breaks this guarantee by introducing `fn name(x: int): string { ... }`, `let x: int = 42;`, and compile-time operator validation.

2. **Library API break — new public types and changed evaluator behavior**  
   `src/lib.rs` exports `Value`, `AstKind`, `Environment`, `Evaluator`. `v2` requires:
   - `AstKind` gains new annotation-carrying variants (`TypedExpression`, `TypedDeclaration`).
   - `Evaluator::evaluate` may return a new `TypeError` diagnostic category (breaking existing exhaustive matches on `AstKind`).
   - `Environment` gains a `TypeContext` parameter (breaking `Environment::new()` signature for library consumers).

3. **CLI break — new flags and changed error reporting**  
   `docs/guarantees.md` guarantees CLI behavior. `v2` introduces:
   - `ucl --type-check <file>` (new mode).
   - `ucl --strict-types` (rejects unannotated functions in strict mode).
   - Error messages for type mismatches replace some runtime error categories with compile-time equivalents.

4. **Resource limits — compile-time budget**  
   The 256 MiB cumulative allocation budget (`v1.15`) applies to evaluation. `v2` introduces a separate **compile-time type-inference budget** (e.g., 64 MiB) for annotation propagation. This is a new safeguard, not a removal, but changes the resource-limit contract.

---

## What does not change (stability preserved)

- **Reserved keyword set** (`Keyword` enum in `src/lexer.rs`): `use`, `if`, `else`, `while`, `for`, `in`, `break`, `continue`, `true`, `false`, `let`, `return`, `fn`. These remain reserved; no identifier reclaim is permitted (`docs/guarantees.md`).
- **Value variants** (`Value` enum in `src/evaluator/value.rs`): `Unit`, `Integer`, `Boolean`, `Str`, `Function`, `Module` remain unchanged in shape; no removal.
- **Module import resolution** (`src/module.rs`): `use "path";` and `use "path" as module;` behavior is preserved exactly.
- **Formatter** (`src/fmt.rs`): `ucl fmt` output format and idempotency guarantees preserved.
- **Resource limits** (`docs/guarantees.md`): The 8 MiB string limit, 100,000-iteration loop cap, and 256 MiB evaluation budget remain intact; only a new compile-time budget is added.

---

## Technical design (high-level)

### Phase 1: Type syntax (`docs/spec.md`, `src/parser.rs`)
- Add type syntax to the grammar: `type ::= "int" | "bool" | "string" | "list" | "function" | "unit" | "module"`.
- Extend `AstKind` with optional annotation spans (additive in minor releases, but the evaluation semantics change is the break).
- Preserve unannotated syntax: existing `v1.16.1` source files parse without modification.

### Phase 2: Type inference (`src/evaluator/` — new `typecheck.rs`)
- Implement compile-time inference for unannotated expressions (optional feature, not required for `v2.0.0` core, but planned).
- Type-check `BinaryOperator` applications (`+`, `-`, `==`, etc.) against operand annotations.
- Ensure `break`/`continue` are only valid inside loops (existing rule preserved; no break).

### Phase 3: Library API (`src/lib.rs`, `src/evaluator/mod.rs`)
- Export `TypeContext`, `TypeAnnotation`, `TypeResult`.
- Modify `Environment::new()` to accept an optional `TypeContext` (signature change = break).
- Add `Evaluator::evaluate_typed()` alongside `evaluate()` for backward-compatibility within `v2` (but `evaluate()` behavior may change for annotated input).

### Phase 4: CLI and diagnostics (`src/main.rs`, `src/diagnostic.rs`)
- New CLI flags (`--type-check`, `--strict-types`).
- New diagnostic severity/category for type errors (additive, but the shift from runtime to compile-time is the break).
- Update `docs/guarantees.md` for `v2`: replace the dynamic-type guarantee with the static-checking contract.

---

## Milestones (suggested, not scheduled)

| Milestone | Deliverable | Break declared |
|-----------|-------------|--------------|
| M1 | Type syntax in parser; `docs/spec.md` updated | Language syntax change |
| M2 | Compile-time operator checking; new `AstKind` variants | Library API (new variants = break for exhaustive matchers) |
| M3 | `Environment::new()` signature change; `TypeContext` exported | Library API (signature change) |
| M4 | CLI flags; new error categories; `CHANGELOG.md` finalized | CLI + diagnostics contract |
| Release `v2.0.0` | `Cargo.toml` = `2.0.0`; `docs/guarantees.md` rewritten; all breaks listed | **Major version bump** |

---

## Compatibility guarantees for v2 (proposed contract)

Starting from `v2.0.0`:

- Programs that pass `ucl --type-check` evaluate with the same result in all later `v2.x` releases (same contract, but stricter entry condition).
- Unannotated programs remain dynamically typed; annotated programs must satisfy compile-time checks.
- The reserved keyword set remains fixed (grows only in future major versions).
- New value variants, built-ins, and syntax may be added in minor `v2.x` releases (same additive guarantee as `v1.x`).

---

## Success criteria

- A `v1.16.1` program without annotations continues to run unmodified under `v2.0.0` (backward compatibility for unannotated code).
- A `v2` annotated program fails at compile time for any operator applied to the wrong annotated type (e.g., `"hello" + 42` with `: int` annotations).
- `CHANGELOG.md` contains at least one `**Breaking**` heading referencing `docs/guarantees.md`.
- `docs/guarantees.md` explicitly states the new static-checking contract and removes the pure dynamic-typing promise.
- The full test suite (`cargo test --all-features`), property tests (`tests/property.rs`), and formatting tests (`tests/formatting.rs`) pass against `v2`.

---

## References

- `docs/guarantees.md` — final v1 compatibility contract (`v1.19.0`)
- `docs/roadmap.md` — completed v1 milestones and the transition to v2
- `docs/spec.md` §3 — current dynamic typing definition
- `CHANGELOG.md` — no breaking changes in stable `1.x` releases
- `Cargo.toml` — `version = "1.19.0"`
