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
use crate::evaluator::{Environment, Evaluator, ModuleValue, Value};
use crate::lexer::{Lexer, unescape_string};
use crate::parser::Parser;
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
}

impl ModuleState {
    /// Returns completed exports when the module at `path` has already run.
    pub(crate) fn exports(&self, path: &Path) -> Option<ModuleValue> {
        self.loaded.get(path).cloned()
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

/// Resolves a decoded module path relative to the importing file's location.
///
/// Falls back to the process working directory when the source has no parent
/// directory, which keeps interactive front ends such as a REPL working.
fn resolve_module_path(source: &SourceFile, raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() {
        return Err("module path is empty".to_owned());
    }

    let name = source.name();
    let base = Path::new(name)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let joined = base.map_or_else(|| PathBuf::from(raw), |base| base.join(raw));

    // Best effort: keep the unresolved path when canonicalization fails so
    // the later read produces a precise "cannot read module" error instead.
    Ok(joined.canonicalize().unwrap_or(joined))
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
        let module_path = match resolve_module_path(source, &decoded) {
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
            let returned_early = self.has_pending_return();
            let module_globals = environment.restore_globals(saved_scopes);

            if module_sink.has_errors() {
                environment.modules.abort();
                report_module_failure(sink, &module_path, &module_sink, *path_span);
                return Value::Unit;
            }
            if returned_early {
                environment.modules.abort();
                sink.emit(Diagnostic::error("`return` outside of a function").at(*path_span));
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
            let path = self.write(file_name, source_text);
            let source = SourceFile::new(path.display().to_string(), source_text);
            let mut sink = DiagnosticSink::new();
            let tokens = Lexer::new(&source).tokenize(&mut sink);
            let value = match Parser::new(tokens).parse(&mut sink) {
                Some(ast) => {
                    Evaluator::new().evaluate_in(&mut Environment::new(), &ast, &source, &mut sink)
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
}
