# UCL v1.x Release Plan — Sequential Capstone Before v2.0.0

**Current version:** `1.18.0` (resolved import-graph tooling)
**Target final v1 release:** `1.19.0` (capstone)  
**Next major version:** `2.0.0` (goal: `docs/v2-goal.md` — static type checking)  
**Plan status:** `1.17.0` and `1.18.0` complete; `1.19.0` remains planned
**Branch:** `main`

---

## Principles (hard rules from `docs/guarantees.md`)

Every `v1.x` release in this plan must follow these rules; violation requires promoting the release to `v2.0.0`:

1. **Additive only** — no `**Breaking**` headings in `CHANGELOG.md`.
2. **Language compatibility preserved** — `docs/spec.md` behavior unchanged; no removal of reserved keywords (`Keyword` enum in `src/lexer.rs`), no change to `Value` variants (`src/evaluator/value.rs`), no change to module resolution (`src/module.rs`).
3. **Library API preserved** — `src/lib.rs` exports (`Value`, `AstKind`, `Environment`, `Evaluator`, etc.) may gain new variants or methods but may not remove or change existing signatures.
4. **CLI preserved** — `ucl fmt`, `ucl -e`, `-p/--path`, `UCL_PATH`, and meta commands (`:help`, `:reset`, `:quit`) behave identically.
5. **Resource limits preserved** — 8 MiB string value, 100,000-iteration loop cap, 256 MiB cumulative allocation budget (`docs/guarantees.md`).
6. **Semantic versioning** — `Cargo.toml` version bumps follow `1.x.0`; MSRV (`rust-version`) may rise but must be noted.

---

## Release sequence

### 1.17.0 — Performance & Evaluator Refinement

**Theme:** Capitalize on the `1.16.1` evaluator decomposition.  
**Target date:** Not scheduled (draft only).  
**Version type:** Minor (`1.17.0` — additive feature / performance improvement).

**Technical focus:**
- Optimize the per-construct evaluation methods introduced in `1.16.1` (`eval_while`, `eval_for`, `eval_assignment`, etc. in `src/evaluator/mod.rs`).
- Profile and reduce overhead in `pending_flow` management (`RefCell<Option<Flow>>`) and resource tracking (`resource_exhausted`).
- Improve list accumulation performance (`Rc<Vec<Value>>` from `1.13`) through buffer pre-allocation or in-place mutation optimization (additive; observable semantics unchanged).
- Ensure formatting (`ucl fmt`) remains idempotent after any parser or AST changes (property tests in `tests/formatting.rs`).

**Deliverables:**
- Performance benchmarks (measured against `v1.16.1`) for loop evaluation, list rebinding, and function call overhead.
- Updated `CHANGELOG.md` with performance improvements (no `Breaking` heading).
- Updated `docs/development.md` with profiling instructions (additive documentation).
- `Cargo.toml`: `version = "1.17.0"`.

**Checklist:**
- [ ] `cargo test --all-features` passes.
- [ ] `tests/property.rs` passes (deterministic pseudo-random generator — reproducible failures).
- [ ] `tests/formatting.rs` passes (idempotency + semantics preservation).
- [ ] `tests/smoke.rs` passes (public API smoke tests).
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo deny check` passes (`deny.toml` — advisories, licenses, duplicates).
- [ ] `docs/` builds with warnings denied (`docs/guarantees.md`, `docs/spec.md`, `docs/development.md`).
- [ ] `CHANGELOG.md` updated (no `Breaking` heading).
- [ ] `README.md` version references updated if applicable.
- [ ] Release artifacts (`sha256sums.txt`) built for Linux x86_64, macOS (`aarch64`, `x86_64`), Windows (`.zip`, `.tar.gz`).

**Break declared:** None.

---

### 1.18.0 — Resolved Import-Graph Tooling

**Theme:** Make module resolution observable without changing import contracts.
**Status:** Completed on 2026-08-28.
**Version type:** Minor (`1.18.0` — additive and backward-compatible).

**Delivered:**
- Added `ucl --list-imports [--path <dir>]... <file>`, which prints the
  canonical root and resolved transitive `importer -> imported` edges in
  deterministic depth-first source order without evaluating UCL code.
- Added `ucl::module::{ImportGraph, ImportGraphEdge, resolved_import_graph}`
  for Rust tooling that needs the same resolution-only traversal.
- Reused the evaluator's established relative lookup, extensionless fallback,
  `-p/--path` and `UCL_PATH` precedence, missing-path reporting, and cycle
  detection rather than introducing a second resolution policy.
- Added end-to-end coverage for transitive traversal, search-path resolution,
  non-evaluation, missing imports, and invalid flag invocations; documented
  the command in the README, development guide, and CLI compatibility contract.

**Break declared:** None. No language syntax, runtime value, import-resolution,
or existing command-line behavior changed.

---

### 1.19.0 — Final v1 Capstone & v2 Transition Preparation

**Theme:** Stabilize, document, and prepare the transition to `v2.0.0`.  
**Target date:** Not scheduled; intended as the final `v1` release before `v2`.  
**Version type:** Minor (`1.19.0` — stability release, no behavior change).

**Technical focus:**
- Final audit of `docs/guarantees.md` against `v1.19.0` behavior (ensure every promise is accurate before `v2` rewrites the contract).
- Final audit of `docs/spec.md` for completeness (verify all `v1.0` through `v1.19.0` features are documented: strings, lists, modules, loops, built-ins, formatter, resource limits, CLI, REPL).
- Update `docs/roadmap.md` to mark `v1` complete and point to `v2` (`docs/v2-goal.md`).
- Verify release artifacts across all supported platforms (Linux x86_64, macOS Apple Silicon/Intel, Windows x86_64) with `sha256sums.txt`.
- Raise MSRV if required (`rust-toolchain.toml`, `Cargo.toml` `rust-version`) — must be noted in `CHANGELOG.md` but is not a `Breaking` change (allowed in minor releases per `docs/guarantees.md`).

**Deliverables:**
- `CHANGELOG.md`: `1.19.0` entry (likely "Internal quality / stability release — no behavior change" or similar, matching `1.16.1` and `1.13.0` patterns).
- `docs/guarantees.md`: Final `v1` contract preserved; `v2` contract documented in `docs/v2-goal.md` (separate file, not yet active).
- `README.md`: Updated to reference `v1.19.0` and preview `v2.0.0` direction (`docs/v2-goal.md`).
- `Cargo.toml`: `version = "1.19.0"`.

**Checklist:**
- [ ] All previous version checklists pass.
- [ ] `docs/v2-goal.md` referenced in `README.md` as the planned major release direction.
- [ ] `CHANGELOG.md` contains no `Breaking` headings for any `1.x` release (`1.0.0` through `1.19.0`).
- [ ] `docs/roadmap.md` updated to show `v1` complete (`[x]` through `1.19.0`) and reference `docs/v2-goal.md`.
- [ ] `docs/development.md` references both `v1` stability and `v2` transition.
- [ ] Full CI matrix passes (Linux, macOS, Windows) — `tests/cli.rs`, `tests/property.rs`, formatting, lint, docs, deny, release build.

**Break declared:** None.

---

## Version summary table

| Version | Type | Theme | Breaking? | Key deliverable |
|---------|------|-------|-----------|-----------------|
| `1.16.1` | Patch | Evaluator decomposition | None | Complete |
| `1.17.0` | Minor | Resource-limit resilience | None | Complete |
| `1.18.0` | Minor | Resolved import-graph tooling | None | CLI and public-library inspection API |
| `1.19.0` | Minor | Final `v1` capstone / `v2` prep | None | `CHANGELOG.md`, `docs/guarantees.md` audit, `README.md`, artifact verification |
| `2.0.0` | **Major** | Static type checking (`docs/v2-goal.md`) | **Yes** — declared in `CHANGELOG.md` (`Breaking` heading) | `docs/guarantees.md` rewritten, `docs/spec.md` updated, `Cargo.toml` = `2.0.0` |

---

## Transition criteria: `1.19.0` → `2.0.0`

`v2.0.0` must NOT be released until all of the following are met:

1. `v1.19.0` is tagged and released with verified artifacts (`sha256sums.txt`).
2. `CHANGELOG.md` for `v2.0.0` contains at least one `**Breaking**` heading referencing `docs/guarantees.md`.
3. `docs/v2-goal.md` is finalized (not draft) and approved.
4. The `arena/01a03a0c-universal-coding-language` branch (or a dedicated release branch) contains a clean `git` state for `v2` work.
5. `Cargo.toml` version is bumped to `"2.0.0"` in the `v2` commit.
6. `docs/guarantees.md` is rewritten with the `v2` contract (static checking) and the old `v1` contract is preserved in a `v1-guarantees.md` archive or clearly marked as superseded.

---

## Reference files

- `docs/guarantees.md` — compatibility rules for all `v1.x` releases.
- `docs/roadmap.md` — completed milestones (`v1` complete as of `1.14`; `1.16.1` extends quality work).
- `CHANGELOG.md` — zero `Breaking` headings since `1.0.0` (must remain true through `1.19.0`).
- `docs/v2-goal.md` — `v2.0.0` target (static type checking) and declared breaks.
- `docs/spec.md` — language specification; `v2` updates required.
- `Cargo.toml` — version, MSRV (`rust-version`), dependencies (`[dependencies]` is empty in `1.16.1`).
- `src/lib.rs`, `src/evaluator/value.rs`, `src/evaluator/mod.rs`, `src/parser.rs`, `src/module.rs`, `src/fmt.rs` — core surfaces that must remain stable in `v1.x`.
