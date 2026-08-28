//! Lexical scopes and binding resolution.
//!
//! The environment is the evaluator's memory: a stack of hash-map scopes
//! plus the read-only built-in prelude and module-loading bookkeeping. All
//! name resolution — lookup, shadowing, capture, and calls — flows through
//! the methods here.

use std::collections::HashMap;

use crate::module::ModuleState;

use super::{BuiltinFunction, TypeContext, Value};

/// A stack of lexical scopes, each mapping names to values.
///
/// Lookups walk the stack from the innermost scope outward, so a block can
/// shadow an outer binding while leaving the outer binding intact.
///
/// The first scope is the *global* scope: it persists for the lifetime of the
/// environment and is what function bodies resolve dynamically. Interactive
/// front ends such as a REPL keep one environment alive across evaluations so
/// bindings survive between inputs.
pub struct Environment {
    /// The stack of user scopes, with the innermost (most recent) scope at the
    /// end. The first scope is the mutable program global scope.
    pub(crate) scopes: Vec<HashMap<String, Value>>,
    /// Read-only built-ins consulted after every user scope. A user declaration
    /// may shadow a built-in, but assignment never mutates the prelude.
    builtins: HashMap<String, Value>,
    /// Compile-time bindings retained across evaluations in this session.
    pub(crate) types: TypeContext,
    /// Module loading bookkeeping: what has been evaluated and what is
    /// currently in progress. Owned logic lives in [`crate::module`].
    pub(crate) modules: ModuleState,
}

impl Environment {
    /// Creates an empty environment containing only the global scope.
    ///
    /// The supplied [`TypeContext`] owns static bindings for the same session.
    /// Passing it explicitly is a v2 API change: embedders can now retain or
    /// reset runtime and compile-time state together with deliberate control.
    pub fn new(types: TypeContext) -> Self {
        Self {
            scopes: vec![HashMap::new()],
            builtins: BuiltinFunction::all()
                .map(|builtin| (builtin.name().to_owned(), Value::Builtin(builtin)))
                .collect(),
            types,
            modules: ModuleState::default(),
        }
    }

    /// Saves the scope stack and installs a fresh one holding only an empty
    /// global scope, isolating subsequent evaluation from existing bindings.
    pub(crate) fn isolate_globals(&mut self) -> Vec<HashMap<String, Value>> {
        std::mem::replace(&mut self.scopes, vec![HashMap::new()])
    }

    /// Appends a directory to the session's module search path.
    ///
    /// `use` statements consult these directories, in insertion order, when a
    /// module cannot be found next to the importing file. Adding paths after
    /// modules have already been evaluated is allowed but only affects later
    /// lookups; completed exports stay cached for the session.
    pub fn add_search_path(&mut self, path: impl Into<std::path::PathBuf>) {
        self.modules.push_search_path(path.into());
    }

    /// Restores a scope stack saved by [`Environment::isolate_globals`] and
    /// returns the bindings of the (replaced) global scope.
    pub(crate) fn restore_globals(
        &mut self,
        saved: Vec<HashMap<String, Value>>,
    ) -> HashMap<String, Value> {
        std::mem::replace(&mut self.scopes, saved)
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// Binds `name` in the global scope. Returns false when the name is
    /// already bound there, leaving the existing binding untouched.
    pub(crate) fn define_global(&mut self, name: String, value: Value) -> bool {
        let global = &mut self.scopes[0];
        if global.contains_key(&name) {
            return false;
        }
        global.insert(name, value);
        true
    }

    /// Returns whether `name` is already bound in the program global scope.
    pub(crate) fn has_global(&self, name: &str) -> bool {
        self.scopes[0].contains_key(name)
    }

    /// Pushes a new, empty scope onto the stack.
    pub(crate) fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pops the innermost scope from the stack.
    pub(crate) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Binds `name` to `value` in the innermost scope.
    ///
    /// If a binding with the same name already exists in the innermost scope,
    /// it is shadowed (replaced) by the new binding.
    pub(crate) fn define(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("the environment always has at least one scope")
            .insert(name.to_owned(), value);
    }

    /// Looks up `name`, searching scopes from innermost outward.
    ///
    /// Returns the first binding found, or `None` if no binding exists.
    pub(crate) fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.builtins.get(name))
    }

    /// Snapshots the non-global bindings visible from innermost to outermost
    /// scope, used as a function literal's capture.
    ///
    /// Inner scopes shadow outer ones; the first binding seen walking outward
    /// wins. Globals are excluded: they resolve dynamically at call time.
    pub(crate) fn capture_non_globals(&self) -> HashMap<String, Value> {
        let mut captured = HashMap::new();
        for scope in self.scopes.iter().skip(1).rev() {
            for (name, value) in scope {
                captured
                    .entry(name.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        captured
    }

    /// Begins a function call with the global scope, the function's captured
    /// bindings, and a fresh parameter scope on top, returning the caller's
    /// scopes for restoration when the call finishes.
    pub(crate) fn begin_call(
        &mut self,
        captured: &HashMap<String, Value>,
    ) -> Vec<HashMap<String, Value>> {
        let mut caller_scopes = std::mem::take(&mut self.scopes);
        let global = caller_scopes.remove(0);
        self.scopes.push(global);
        let captured = captured
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        self.scopes.push(captured);
        self.push_scope();
        caller_scopes
    }

    /// Restores caller scopes after a function call while preserving changes
    /// the function made to global bindings.
    pub(crate) fn end_call(&mut self, mut caller_scopes: Vec<HashMap<String, Value>>) {
        let global = self.scopes.remove(0);
        caller_scopes.insert(0, global);
        self.scopes = caller_scopes;
    }

    /// Reassigns an existing binding. Returns `false` if `name` is unbound.
    ///
    /// The assignment searches scopes from innermost to outermost and updates
    /// the first binding found. This allows shadowed bindings to be updated
    /// correctly.
    pub(crate) fn assign(&mut self, name: &str, value: Value) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                *slot = value;
                return true;
            }
        }
        false
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new(TypeContext::new())
    }
}
