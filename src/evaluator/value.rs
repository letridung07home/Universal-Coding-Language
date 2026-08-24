//! Runtime values produced by evaluating a program.
//!
//! Every expression evaluates to exactly one [`Value`]. The variants that
//! carry behavior — functions, built-ins, and module namespaces — are
//! opaque to UCL code: their internals stay private so the language's
//! guarantees about mutation and equality hold.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::parser::AstNode;
use crate::source::SourceFile;

use super::BuiltinFunction;

/// A read-only collection of exported module bindings.
///
/// Namespace members are created by `use "path" as alias;` and are resolved
/// with `alias.member` expressions. The export map is kept private so UCL code
/// cannot mutate a module namespace after it has been evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleValue {
    exports: BTreeMap<String, Value>,
}

impl ModuleValue {
    /// Creates a module namespace from its completed top-level exports.
    pub(crate) fn new(exports: BTreeMap<String, Value>) -> Self {
        Self { exports }
    }

    /// Looks up an exported binding by name.
    pub(crate) fn get(&self, name: &str) -> Option<&Value> {
        self.exports.get(name)
    }

    /// Iterates over the namespace's exported bindings in deterministic order.
    pub(crate) fn exports(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.exports.iter()
    }
}

/// A runtime value produced by evaluating a program.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// No value (unit).
    ///
    /// This is the result of declarations and other statements that don't
    /// produce a value.
    Unit,
    /// A signed 64-bit integer.
    Integer(i64),
    /// A boolean value (`true` or `false`).
    Boolean(bool),
    /// A string of Unicode text.
    Str(String),
    /// A named UCL function that can be called with positional arguments.
    Function(FunctionValue),
    /// A callable supplied by the built-in prelude.
    Builtin(BuiltinFunction),
    /// A read-only namespace produced by an aliased module import.
    Module(ModuleValue),
    /// An ordered, immutable sequence of values.
    ///
    /// The elements live behind an `Rc` so cloning a list value — rebinding
    /// it, capturing it in a closure, passing it as an argument — copies a
    /// reference instead of every element. Functional updates (`append`,
    /// concatenation, slicing) reuse the shared buffer through
    /// [`Rc::make_mut`] when the list is not aliased, which keeps the
    /// accumulate-in-a-loop idiom linear.
    List(Rc<Vec<Value>>),
}

/// A callable UCL function value.
///
/// A function executes with the global scope, the bindings it *captured*
/// when it was created, and a fresh parameter scope. Globals are resolved
/// dynamically at call time — which is what lets top-level functions
/// recurse by looking themselves up — while non-global bindings visible at
/// creation are captured by value: later changes to those bindings do not
/// affect an already-created function.
#[derive(Clone, Debug)]
pub struct FunctionValue {
    pub(crate) parameters: Vec<String>,
    pub(crate) body: AstNode,
    pub(crate) captured: HashMap<String, Value>,
    /// The source the function was defined in. Function bodies are evaluated
    /// against their own text so that a function value remains valid when
    /// called from a different source — most importantly from a later line
    /// of an interactive session.
    pub(crate) source: std::sync::Arc<SourceFile>,
}

/// Two function values are equal when they were created from equal code,
/// parameters, and captures; the defining source's identity is irrelevant.
impl PartialEq for FunctionValue {
    fn eq(&self, other: &Self) -> bool {
        self.parameters == other.parameters
            && self.body == other.body
            && self.captured == other.captured
    }
}

impl Value {
    /// Returns the value's short type name, used in diagnostics and by
    /// the `type` built-in.
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "unit",
            Value::Integer(_) => "integer",
            Value::Boolean(_) => "boolean",
            Value::Str(_) => "string",
            Value::Function(_) | Value::Builtin(_) => "function",
            Value::Module(_) => "module",
            Value::List(_) => "list",
        }
    }

    /// Renders the value as text, exactly as the CLI and REPL echo results.
    ///
    /// Unit renders as `"unit"`, matching its type name; interactive front
    /// ends omit unit results instead of printing this text. Strings render
    /// their raw contents, so `str` of a string is that string.
    pub fn display_text(&self) -> String {
        match self {
            Value::Unit => self.type_name().to_owned(),
            Value::Integer(integer) => format!("{integer}"),
            Value::Boolean(boolean) => format!("{boolean}"),
            Value::Str(string) => string.clone(),
            Value::Function(_) | Value::Builtin(_) => "<function>".to_owned(),
            Value::Module(_) => "<module>".to_owned(),
            Value::List(elements) => {
                let rendered = elements
                    .iter()
                    .map(|element| element.element_text())
                    .collect::<Vec<String>>()
                    .join(", ");
                format!("[{rendered}]")
            }
        }
    }

    /// Renders the value the way it appears *inside* an echoed list.
    ///
    /// Strings are quoted so list contents stay unambiguous; everything
    /// else renders exactly as it does at top level.
    fn element_text(&self) -> String {
        match self {
            Value::Str(string) => format!("\"{string}\""),
            other => other.display_text(),
        }
    }
}
