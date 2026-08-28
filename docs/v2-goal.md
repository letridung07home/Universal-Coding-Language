# UCL v2.0.0: Optional Static Type Checking

**Initial release:** `2.0.0`
**Current v2 release:** `2.0.1`
**Status:** Implemented and maintained
**Branch:** `main`

## Release goal

UCL `v2.0.0` introduced **optional static type annotations** and
**compile-time checking** while preserving dynamic evaluation for programs that
do not opt in. The feature provides an incremental migration path: existing v1
source continues to parse and follows its established runtime semantics, while
annotated source is validated before it runs.

> A program with type annotations is rejected before evaluation when UCL can
> prove that its declarations, operators, calls, conditions, indexes, supported
> built-in calls, or function result conflict with the declared type.

## Syntax and checking model

The seven contextual source type names are `int`, `bool`, `string`, `list`,
`function`, `unit`, and `module`. A type name is special only after `:`, so
`let int = 42;` remains legal v1-compatible source.

```ucl
fn twice(value: int): int {
    value + value;
};

let answer: int = twice(21);
```

Annotations are accepted on `let` declarations, function parameters, and
function return types. A program containing annotations is checked before
normal evaluation. Type checking remains deliberately conservative: values with
no declared or inferable type stay dynamic rather than being assigned an
unsound type. The `--strict-types` mode requires every function to provide a
fully annotated parameter and return signature.

| Invocation | Behavior | Evaluation occurs? |
|---|---|---|
| `ucl program.ucl` | Checks annotated source, then evaluates it | Yes, if checking succeeds |
| `ucl --type-check program.ucl` | Checks source and reports static errors | No |
| `ucl --type-check --strict-types program.ucl` | Strictly checks source only | No |
| `ucl --strict-types program.ucl` | Strictly checks source, then evaluates it | Yes, if checking succeeds |

## Deliberate breaking changes

### Breaking

1. **Language contract.** The v1 promise that UCL performs no static type
   checking is replaced by optional annotations and pre-evaluation type errors.
   Unannotated code remains dynamic; annotated code can now fail earlier than
   the equivalent v1 source.
2. **AST and library API.** `AstKind::Let` exposes an optional annotation and
   `AstKind::Function` now exposes `Parameter` values and an optional return
   annotation. New public `Type`, `TypeContext`, `TypeName`, and
   `TypeAnnotation` types support embedded checking.
3. **Environment construction.** `Environment::new()` is replaced by
   `Environment::new(TypeContext)`. `Environment::default()` offers a concise
   fresh-context construction path.
4. **CLI surface.** `--type-check` and `--strict-types` add static checking
   modes. `--list-imports` cannot be combined with those modes because it is a
   dedicated import-resolution inspection operation.
5. **Compile-time safeguard.** Static checking has an independent budget of
   1,000,000 AST-node visits, preventing an annotated input from consuming
   unbounded compiler work.

## Preserved behavior

The v1 reserved-keyword set is unchanged, module resolution retains its
relative-first, extensionless, `-p/--path`, and `UCL_PATH` behavior, and the
formatter remains deterministic and idempotent. Runtime safeguards—the 8 MiB
string limit, 100,000-per-loop and 1,000,000-total-loop caps, and the 256 MiB
value-allocation budget—are unchanged.

## Acceptance criteria

| Criterion | Release-candidate status |
|---|---|
| Unannotated v1 programs retain dynamic evaluation | Implemented and regression-tested |
| Annotated declarations and typed functions reject known mismatches | Implemented and tested |
| `--type-check` performs no evaluation | Implemented and tested |
| `--strict-types` enforces complete function signatures | Implemented and tested |
| Type annotations format canonically | Implemented and verified |
| v2 guarantees replace the v1 dynamic-only contract | Documented in `docs/guarantees.md` |
| Final v1 contract remains available | Archived in `docs/v1-guarantees.md` |
