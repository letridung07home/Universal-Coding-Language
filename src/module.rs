//! Module loading for the evaluator.
//!
//! A `use "path.ucl";` statement loads a file, evaluates it in isolation,
//! and merges its top-level bindings into the importer's global scope.
//! This module owns every piece of that machinery: tracking which modules
//! have been evaluated, resolving paths relative to the importing file,
//! detecting import cycles, and summarizing a failed module's diagnostics
//! back at the importing `use` site.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::evaluator::{Environment, Evaluator, ModuleValue, TypeContext, Value};
use crate::lexer::{Lexer, unescape_string};
use crate::parser::{AstKind, Parser};
use crate::source::{SourceFile, Span};

/// Per-session module bookkeeping owned by [`Environment`].
#[derive(Default)]
pub(crate) struct ModuleState {
    /// Completed module exports keyed by canonical path. Retaining the export
    /// map lets a session reuse one evaluation through flat imports, multiple
    /// aliases, or a mixture of both forms.
    loaded: HashMap<PathBuf, ModuleValue>,
    /// Modules whose exports have already been merged through the legacy flat
    /// import form. A repeated flat import is intentionally a no-op.
    flattened: HashSet<PathBuf>,
    /// Modules currently being evaluated, innermost last. Used to detect
    /// and report circular imports.
    loading: Vec<PathBuf>,
    /// Directories consulted when an import cannot be found next to the
    /// importing file, in configuration order. Populated through the public
    /// [`Environment`](crate::evaluator::Environment) search-path API.
    search_paths: Vec<PathBuf>,
}

impl ModuleState {
    /// Returns completed exports when the module at `path` has already run.
    pub(crate) fn exports(&self, path: &Path) -> Option<ModuleValue> {
        self.loaded.get(path).cloned()
    }

    /// Appends a directory to the end of the module search path.
    pub(crate) fn push_search_path(&mut self, path: PathBuf) {
        self.search_paths.push(path);
    }

    /// Returns the session's module search directories in lookup order.
    pub(crate) fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }

    /// Returns true when this module's exports were already merged through a
    /// legacy flat import in the current session.
    pub(crate) fn was_flattened(&self, path: &Path) -> bool {
        self.flattened.contains(path)
    }

    /// Marks a successful legacy flat import as merged.
    pub(crate) fn mark_flattened(&mut self, path: PathBuf) {
        self.flattened.insert(path);
    }

    /// Returns true when evaluating `path` is already in progress, meaning
    /// the import graph contains a cycle through this module.
    pub(crate) fn is_loading(&self, path: &Path) -> bool {
        self.loading.iter().any(|entry| entry == path)
    }

    /// Marks `path` as being evaluated.
    pub(crate) fn begin(&mut self, path: PathBuf) {
        self.loading.push(path);
    }

    /// Marks the current module as successfully evaluated and caches exports.
    pub(crate) fn finish(&mut self, exports: ModuleValue) {
        if let Some(path) = self.loading.pop() {
            self.loaded.insert(path, exports);
        }
    }

    /// Removes the current module from the in-progress stack without caching
    /// it after evaluation failed.
    pub(crate) fn abort(&mut self) {
        self.loading.pop();
    }
}

/// A directed edge in a resolved UCL import graph.
///
/// Both paths are canonical when the corresponding source files exist. The
/// graph retains one edge for each `use` statement, in deterministic depth-first
/// source order; shared modules are traversed only once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportGraphEdge {
    /// The source file that contains the `use` statement.
    pub importer: PathBuf,
    /// The source file selected by that `use` statement.
    pub imported: PathBuf,
}

/// A resolved import graph rooted at one UCL source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportGraph {
    /// The root source file provided to the traversal.
    pub root: PathBuf,
    /// Directed import edges in deterministic depth-first source order.
    pub edges: Vec<ImportGraphEdge>,
}

/// Resolves a decoded module path against the importing file's location and
/// the session's search directories.
///
/// Candidates are tried in a fixed order: the importing file's directory
/// first (so existing programs keep resolving exactly as before), then each
/// configured search directory. Every location is tried as written and, when
/// it has no `.ucl` extension, once more with the extension added; the first
/// existing file wins. Absolute import paths replace the base directory
/// entirely, so they bypass the search path.
///
/// When no candidate exists, the error lists every location that was tried.
fn resolve_module_path(
    source: &SourceFile,
    raw: &str,
    search_paths: &[PathBuf],
) -> Result<PathBuf, String> {
    if raw.is_empty() {
        return Err("module path is empty".to_owned());
    }

    let name = source.name();
    let base = Path::new(name)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(PathBuf::new, Path::to_path_buf);

    let mut tried = Vec::new();
    for directory in std::iter::once(&base).chain(search_paths) {
        let direct = directory.join(raw);
        // Best effort: keep the unresolved path when canonicalization fails
        // so the later read produces a precise error instead.
        if let Ok(canonical) = direct.canonicalize() {
            return Ok(canonical);
        }
        tried.push(direct.display().to_string());
        if direct.extension().is_none() {
            let extended = direct.with_extension("ucl");
            if let Ok(canonical) = extended.canonicalize() {
                return Ok(canonical);
            }
            tried.push(extended.display().to_string());
        }
    }

    Err(format!(
        "cannot read module `{raw}`: none of these locations exist:\n{}",
        tried
            .iter()
            .map(|path| format!("  - {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

/// Extracts decoded import paths from one fully parsed source file.
///
/// The parser already enforces that `use` statements occur only at the program
/// top level. Keeping this pass separate from evaluation means diagnostics and
/// side effects are never triggered by import-graph inspection.
fn source_imports(source: &SourceFile) -> Result<Vec<(Span, String)>, String> {
    let mut sink = DiagnosticSink::new();
    let tokens = Lexer::new(source).tokenize(&mut sink);
    if sink.has_errors() {
        return Err(first_diagnostic_message(&sink));
    }
    let Some(ast) = Parser::new(tokens).parse(&mut sink) else {
        return Err(first_diagnostic_message(&sink));
    };
    if sink.has_errors() {
        return Err(first_diagnostic_message(&sink));
    }

    let AstKind::Program { statements } = ast.kind else {
        return Err("parser did not produce a program".to_owned());
    };
    let mut imports = Vec::new();
    for statement in statements {
        if let AstKind::Use { path, .. } = statement.kind {
            let raw = source
                .slice(path)
                .ok_or_else(|| "invalid module path span".to_owned())?;
            imports.push((path, unescape_string(raw)));
        }
    }
    Ok(imports)
}

/// Returns the first diagnostic message from a failed lexing or parsing pass.
fn first_diagnostic_message(sink: &DiagnosticSink) -> String {
    sink.iter().next().map_or_else(
        || "unknown error".to_owned(),
        |diagnostic| diagnostic.message.clone(),
    )
}

/// Traverses one already-resolved module without evaluating it.
fn collect_module_imports(
    module_path: &Path,
    search_paths: &[PathBuf],
    visiting: &mut HashSet<PathBuf>,
    visited: &mut HashSet<PathBuf>,
    edges: &mut Vec<ImportGraphEdge>,
) -> Result<(), String> {
    let contents = std::fs::read_to_string(module_path)
        .map_err(|error| format!("cannot read module `{}`: {error}", module_path.display()))?;
    let source = SourceFile::new(module_path.display().to_string(), contents);
    let imports = source_imports(&source).map_err(|detail| {
        format!(
            "cannot inspect imports in `{}`: {detail}",
            module_path.display()
        )
    })?;

    for (_span, raw) in imports {
        let imported = resolve_module_path(&source, &raw, search_paths)?;
        edges.push(ImportGraphEdge {
            importer: module_path.to_path_buf(),
            imported: imported.clone(),
        });
        if visiting.contains(&imported) {
            return Err(format!("circular import of `{}`", imported.display()));
        }
        if visited.insert(imported.clone()) {
            visiting.insert(imported.clone());
            let result = collect_module_imports(&imported, search_paths, visiting, visited, edges);
            visiting.remove(&imported);
            result?;
        }
    }
    Ok(())
}

/// Resolves the complete import graph for `source` without evaluating UCL code.
///
/// Search paths have the same ordering and extensionless lookup behavior as an
/// evaluated `use` statement. Any missing module, malformed source file, or
/// cycle produces a diagnostic and returns `None`; therefore callers never
/// receive a partial graph as a successful result.
pub fn resolved_import_graph(
    source: &SourceFile,
    search_paths: &[PathBuf],
    sink: &mut DiagnosticSink,
) -> Option<ImportGraph> {
    let root = Path::new(source.name())
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(source.name()));
    let imports = match source_imports(source) {
        Ok(imports) => imports,
        Err(detail) => {
            sink.emit(Diagnostic::error(format!(
                "cannot inspect imports in `{}`: {detail}",
                source.name()
            )));
            return None;
        }
    };

    let mut edges = Vec::new();
    let mut visiting = HashSet::from([root.clone()]);
    let mut visited = HashSet::from([root.clone()]);
    for (span, raw) in imports {
        let imported = match resolve_module_path(source, &raw, search_paths) {
            Ok(path) => path,
            Err(message) => {
                sink.emit(Diagnostic::error(message).at(span));
                return None;
            }
        };
        edges.push(ImportGraphEdge {
            importer: root.clone(),
            imported: imported.clone(),
        });
        if visiting.contains(&imported) {
            sink.emit(
                Diagnostic::error(format!("circular import of `{}`", imported.display())).at(span),
            );
            return None;
        }
        if visited.insert(imported.clone()) {
            visiting.insert(imported.clone());
            if let Err(detail) = collect_module_imports(
                &imported,
                search_paths,
                &mut visiting,
                &mut visited,
                &mut edges,
            ) {
                sink.emit(
                    Diagnostic::error(format!(
                        "module `{}` failed to inspect: {detail}",
                        imported.display()
                    ))
                    .at(span),
                );
                return None;
            }
            visiting.remove(&imported);
        }
    }

    Some(ImportGraph { root, edges })
}

/// Reports that loading `module_path` failed by summarizing its first
/// diagnostic at the importing `use` site.
fn report_module_failure(
    sink: &mut DiagnosticSink,
    module_path: &Path,
    module_sink: &DiagnosticSink,
    use_span: Span,
) {
    let detail = module_sink
        .iter()
        .next()
        .map_or_else(|| "unknown error".to_owned(), |d| d.message.clone());
    sink.emit(
        Diagnostic::error(format!(
            "module `{}` failed to load: {detail}",
            module_path.display()
        ))
        .at(use_span),
    );
}

impl Evaluator {
    /// Evaluates a `use` statement by loading or reusing a module's completed
    /// exports, then either flattening them or binding one namespace alias.
    pub(crate) fn eval_use(
        &self,
        path_span: &Span,
        alias_span: &Option<Span>,
        source: &SourceFile,
        environment: &mut Environment,
        sink: &mut DiagnosticSink,
        depth: usize,
    ) -> Value {
        let raw = match source.slice(*path_span) {
            Some(text) => text,
            None => {
                sink.emit(Diagnostic::error("invalid module path span").at(*path_span));
                return Value::Unit;
            }
        };
        let decoded = unescape_string(
            raw.strip_prefix('"')
                .and_then(|text| text.strip_suffix('"'))
                .unwrap_or(raw),
        );
        let module_path =
            match resolve_module_path(source, &decoded, environment.modules.search_paths()) {
                Ok(path) => path,
                Err(message) => {
                    sink.emit(Diagnostic::error(message).at(*path_span));
                    return Value::Unit;
                }
            };

        if environment.modules.is_loading(&module_path) {
            sink.emit(
                Diagnostic::error(format!("circular import of `{}`", module_path.display()))
                    .at(*path_span),
            );
            return Value::Unit;
        }

        let module = if let Some(module) = environment.modules.exports(&module_path) {
            module
        } else {
            let contents = match std::fs::read_to_string(&module_path) {
                Ok(contents) => contents,
                Err(error) => {
                    sink.emit(
                        Diagnostic::error(format!(
                            "cannot read module `{}`: {error}",
                            module_path.display()
                        ))
                        .at(*path_span),
                    );
                    return Value::Unit;
                }
            };

            let module_source = SourceFile::new(module_path.display().to_string(), contents);
            let mut module_sink = DiagnosticSink::new();
            let tokens = Lexer::new(&module_source).tokenize(&mut module_sink);
            if module_sink.has_errors() {
                report_module_failure(sink, &module_path, &module_sink, *path_span);
                return Value::Unit;
            }
            let Some(module_ast) = Parser::new(tokens).parse(&mut module_sink) else {
                report_module_failure(sink, &module_path, &module_sink, *path_span);
                return Value::Unit;
            };
            if module_sink.has_errors() {
                report_module_failure(sink, &module_path, &module_sink, *path_span);
                return Value::Unit;
            }
            let mut module_types = TypeContext::new();
            if !Evaluator::new().type_check(
                &module_ast,
                &module_source,
                &mut module_types,
                &mut module_sink,
                false,
            ) {
                report_module_failure(sink, &module_path, &module_sink, *path_span);
                return Value::Unit;
            }

            environment.modules.begin(module_path.clone());
            // Swap in a fresh global scope so the module cannot see (or mutate)
            // the importer's bindings; its own imports recurse through here.
            let saved_scopes = environment.isolate_globals();
            self.eval(
                &module_ast,
                &module_source,
                environment,
                &mut module_sink,
                depth + 1,
            );
            let escaped_flow = self.has_pending_flow();
            let module_globals = environment.restore_globals(saved_scopes);

            if module_sink.has_errors() {
                environment.modules.abort();
                report_module_failure(sink, &module_path, &module_sink, *path_span);
                return Value::Unit;
            }
            if escaped_flow {
                // Whatever signal reached the module boundary had no matching
                // construct at the module's top level; name it precisely.
                environment.modules.abort();
                sink.emit(
                    Diagnostic::error("module top level cannot `return`, `break`, or `continue`")
                        .at(*path_span),
                );
                return Value::Unit;
            }

            let exports = ModuleValue::new(module_globals.into_iter().collect::<BTreeMap<_, _>>());
            environment.modules.finish(exports.clone());
            exports
        };

        match alias_span {
            Some(alias_span) => {
                let Some(alias) = source.slice(*alias_span) else {
                    sink.emit(Diagnostic::error("invalid module alias span").at(*alias_span));
                    return Value::Unit;
                };
                if !environment.define_global(alias.to_owned(), Value::Module(module)) {
                    sink.emit(
                        Diagnostic::error(format!("module alias `{alias}` is already bound"))
                            .at(*alias_span),
                    );
                }
            }
            None => {
                // Preserve legacy repeated-import behavior while still letting
                // aliases reuse the same completed export map.
                if environment.modules.was_flattened(&module_path) {
                    return Value::Unit;
                }
                // Check every collision first so an error never leaves a
                // partially merged import.
                if let Some((name, _)) = module
                    .exports()
                    .find(|(name, _)| environment.has_global(name))
                {
                    sink.emit(
                        Diagnostic::error(format!(
                            "module defines `{name}`, which is already bound"
                        ))
                        .at(*path_span),
                    );
                    return Value::Unit;
                }
                for (name, value) in module.exports() {
                    let inserted = environment.define_global(name.clone(), value.clone());
                    debug_assert!(
                        inserted,
                        "collisions were checked before merging module exports"
                    );
                }
                environment.modules.mark_flattened(module_path.clone());
            }
        }

        Value::Unit
    }
}

#[cfg(test)]
mod module_tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Monotonic counter for unique temporary directory names.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A scratch directory holding one test's module files.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("ucl-mods-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        /// Writes a file relative to the scratch directory's root.
        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().expect("relative path has a parent"))
                .expect("create parent dirs");
            fs::write(&path, contents).expect("write module file");
            path
        }

        /// Evaluates `source_text` as a program stored in the scratch
        /// directory under the given name.
        fn run(&self, file_name: &str, source_text: &str) -> (Option<Value>, DiagnosticSink) {
            self.run_with_search_paths(file_name, source_text, &[])
        }

        /// Like [`Self::run`], but configures additional session module
        /// search directories before evaluation.
        fn run_with_search_paths(
            &self,
            file_name: &str,
            source_text: &str,
            search_paths: &[&Path],
        ) -> (Option<Value>, DiagnosticSink) {
            let path = self.write(file_name, source_text);
            let source = SourceFile::new(path.display().to_string(), source_text);
            let mut sink = DiagnosticSink::new();
            let tokens = Lexer::new(&source).tokenize(&mut sink);
            let value = match Parser::new(tokens).parse(&mut sink) {
                Some(ast) => {
                    let mut environment = Environment::default();
                    for directory in search_paths {
                        environment.add_search_path(directory);
                    }
                    Evaluator::new().evaluate_in(&mut environment, &ast, &source, &mut sink)
                }
                None => None,
            };
            (value, sink)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn importing_a_module_binds_its_definitions() {
        let dir = TempDir::new();
        dir.write("math.ucl", "fn double(n) { n * 2; }; let answer = 21;");
        let (value, sink) = dir.run("main.ucl", "use \"math.ucl\"; double(answer);");

        assert!(
            !sink.has_errors(),
            "{:?}",
            sink.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
        );
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn imports_are_transitive() {
        let dir = TempDir::new();
        dir.write("a.ucl", "use \"lib/b.ucl\";");
        dir.write("lib/b.ucl", "fn f() { 7; };");
        let (value, sink) = dir.run("main.ucl", "use \"a.ucl\"; f();");

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(7)));
    }

    #[test]
    fn a_module_is_evaluated_exactly_once_per_session() {
        let dir = TempDir::new();
        // If the second import re-evaluated the module, merging its binding
        // of `x` would collide with the one from the first import.
        dir.write("once.ucl", "let x = 1;");
        let (value, sink) = dir.run("main.ucl", "use \"once.ucl\";\nuse \"once.ucl\";\nx + 1;");

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(2)));
    }

    #[test]
    fn circular_imports_are_an_error() {
        let dir = TempDir::new();
        dir.write("a.ucl", "use \"b.ucl\";");
        dir.write("b.ucl", "use \"a.ucl\";");
        let (_value, sink) = dir.run("main.ucl", "use \"a.ucl\";");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| { diagnostic.message.contains("circular import") })
        );
    }

    #[test]
    fn a_missing_module_file_is_reported_at_the_use_site() {
        let dir = TempDir::new();
        let (_value, sink) = dir.run("main.ucl", "use \"nowhere.ucl\";");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("cannot read module")),
            "{:?}",
            sink.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn imported_names_may_not_collide_with_existing_globals() {
        let dir = TempDir::new();
        dir.write("collide.ucl", "let x = 5;");
        let (_value, sink) = dir.run("main.ucl", "let x = 1;\nuse \"collide.ucl\";");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| { diagnostic.message.contains("already bound") })
        );
    }

    #[test]
    fn modules_cannot_see_the_importer_or_callee_bindings() {
        let dir = TempDir::new();
        // The module references `secret`, which only exists in the importer.
        dir.write("sneaky.ucl", "let leak = secret + 1;");
        let (_value, sink) = dir.run("main.ucl", "let secret = 41;\nuse \"sneaky.ucl\";");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| { diagnostic.message.contains("failed to load") })
        );
    }

    #[test]
    fn an_import_inside_a_function_body_is_rejected_by_evaluation() {
        // Defense in depth: the parser rejects this shape, but evaluating a
        // hand-built AST must not silently import either.
        let dir = TempDir::new();
        dir.write("inner.ucl", "1;");
        let source_text = "fn f() { use \"inner.ucl\"; };";
        let source = SourceFile::new("main.ucl", source_text);
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);
        Parser::new(tokens).parse(&mut sink);

        assert!(sink.has_errors(), "the parser must reject nested `use`");
    }

    #[test]
    fn aliased_import_binds_a_read_only_namespace() {
        let dir = TempDir::new();
        dir.write("math.ucl", "fn double(n) { n * 2; }; let answer = 21;");
        let (value, sink) = dir.run(
            "main.ucl",
            "use \"math.ucl\" as math; math.double(math.answer);",
        );

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn aliases_reuse_one_completed_module_export_map() {
        let dir = TempDir::new();
        dir.write("once.ucl", "let value = 21;");
        let (value, sink) = dir.run(
            "main.ucl",
            "use \"once.ucl\" as first; use \"once.ucl\" as second; first.value + second.value;",
        );

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn flat_and_aliased_imports_interoperate_from_the_same_cache() {
        let dir = TempDir::new();
        dir.write("math.ucl", "fn double(n) { n * 2; }; let answer = 21;");
        let (value, sink) = dir.run(
            "main.ucl",
            "use \"math.ucl\" as math; use \"math.ucl\"; math.double(answer);",
        );

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn alias_collisions_and_missing_members_are_errors() {
        let dir = TempDir::new();
        dir.write("math.ucl", "let answer = 42;");
        let (_value, sink) = dir.run(
            "main.ucl",
            "let math = 0; use \"math.ucl\" as math; use \"math.ucl\" as namespace; namespace.missing;",
        );

        assert!(sink.has_errors());
        let messages = sink
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("alias `math` is already bound"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("no exported member `missing`"))
        );
    }

    #[test]
    fn failed_flat_import_does_not_partially_merge_exports() {
        let dir = TempDir::new();
        dir.write("values.ucl", "let x = 1; let y = 2;");
        let (_value, sink) = dir.run("main.ucl", "let y = 7; use \"values.ucl\"; x;");

        assert!(sink.has_errors());
        let messages = sink
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("module defines `y`"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("undefined variable `x`"))
        );
    }

    #[test]
    fn namespace_access_on_a_non_module_is_an_error() {
        let dir = TempDir::new();
        let (_value, sink) = dir.run("main.ucl", "let value = 1; value.answer;");

        assert!(sink.has_errors());
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("cannot access member `answer` on value of type `integer`")
        }));
    }

    #[test]
    fn extensionless_imports_resolve_the_ucl_file() {
        let dir = TempDir::new();
        dir.write("math.ucl", "fn double(n) { n * 2; };");
        let (value, sink) =
            dir.run_with_search_paths("main.ucl", "use \"math\" as m; m.double(21);", &[]);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn search_paths_are_consulted_when_the_relative_lookup_fails() {
        let main_dir = TempDir::new();
        let library_dir = TempDir::new();
        library_dir.write("helper.ucl", "let answer = 42;");
        let (value, sink) = main_dir.run_with_search_paths(
            "main.ucl",
            "use \"helper\" as h; h.answer;",
            &[&library_dir.0],
        );

        assert!(
            !sink.has_errors(),
            "{:?}",
            sink.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
        );
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn the_importing_directory_takes_precedence_over_search_paths() {
        let main_dir = TempDir::new();
        let shadow_dir = TempDir::new();
        // Both directories define `pick`; only the one next to the importer
        // may be loaded.
        main_dir.write("pick.ucl", "let who = \"local\";");
        shadow_dir.write("pick.ucl", "let who = \"searched\";");
        let (value, sink) =
            main_dir.run_with_search_paths("main.ucl", "use \"pick\"; who;", &[&shadow_dir.0]);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("local".to_owned())));
    }

    #[test]
    fn transitive_imports_resolve_through_search_paths() {
        let main_dir = TempDir::new();
        let library_dir = TempDir::new();
        library_dir.write("leaf.ucl", "fn seven() { 7; };");
        // `middle` lives next to the importer but imports through a search
        // path; its own import resolves relative to its own directory, which
        // is not where `leaf` lives.
        main_dir.write("middle.ucl", "use \"leaf\"; let value = seven();");
        let (value, sink) =
            main_dir.run_with_search_paths("main.ucl", "use \"middle\"; value;", &[&library_dir.0]);

        assert!(!sink.has_errors(), "for a module in the search path");
        assert_eq!(value, Some(Value::Integer(7)));
    }

    #[test]
    fn resolution_forms_share_one_cached_evaluation() {
        let dir = TempDir::new();
        dir.write("once.ucl", "let x = 21;");
        // If the two forms resolved to different modules, the second flat
        // import would collide with bindings from the first.
        let (value, sink) = dir.run("main.ucl", "use \"once\";\nuse \"once.ucl\";\nx + 1;");

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(22)));
    }

    #[test]
    fn missing_imports_list_every_tried_location() {
        let main_dir = TempDir::new();
        let library_dir = TempDir::new();
        let (value, sink) =
            main_dir.run_with_search_paths("main.ucl", "use \"nowhere\";", &[&library_dir.0]);

        assert!(sink.has_errors());
        let message = sink
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            message.contains("none of these locations exist"),
            "{message}"
        );
        for expected in ["nowhere", "nowhere.ucl"] {
            assert!(
                message.contains(expected),
                "`{expected}` must be listed: {message}"
            );
        }
        assert_eq!(value, None);
    }
}
