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

- [ ] Define keyword, assignment, operator, and runtime-error semantics
- [ ] Decide whether UCL will be statically or dynamically typed
- [x] Add boolean literals and equality operators
- [ ] Add strings and string operations
- [ ] Add functions, parameters, return values, and closures
- [ ] Add conditional and looping constructs

## Tooling and ecosystem

- [x] Add property tests and fuzz targets for the lexer and parser
- [ ] Publish diagnostic and compatibility guarantees
- [ ] Design a module and package system
- [ ] Add an interactive REPL
- [x] Provide release artifacts and installation instructions

## Completed foundation

- [x] Write the initial language specification
- [x] Implement the lexer
- [x] Implement the parser and abstract syntax tree
- [x] Implement the evaluator
- [x] Add the `ucl` command-line interface
- [x] Render diagnostics with source excerpts
- [x] Add integration tests and CI checks
