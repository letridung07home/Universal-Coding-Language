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

/// A map key: a string or a signed 64-bit integer.
///
/// Keys are restricted to these two value shapes so lookups are total and
/// order-independent. The variant carries the key's data directly so the
/// lookup index can hash keys without inspecting whole [`Value`]s.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum MapKey {
    /// A string key.
    Str(String),
    /// An integer key.
    Int(i64),
}

impl MapKey {
    /// Builds a key from a runtime value, if the value is a legal key.
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Str(text) => Some(Self::Str(text.clone())),
            Value::Integer(number) => Some(Self::Int(*number)),
            _ => None,
        }
    }

    /// Returns the key as a runtime value for iteration and inspection.
    fn to_value(&self) -> Value {
        match self {
            Self::Str(text) => Value::Str(text.clone()),
            Self::Int(number) => Value::Integer(*number),
        }
    }
}

/// An insertion-ordered immutable map from string or integer keys to values.
///
/// Entries live behind an `Rc` so cloning a map value copies a reference
/// instead of every entry, mirroring [`Value::List`]. Iteration and
/// rendering follow insertion order, which keeps programs deterministic;
/// equality ignores order and compares keys to values. Functional updates
/// (`insert`, `remove`) reuse the shared buffer through [`Rc::make_mut`]
/// when the map is not aliased, which keeps the accumulate-in-a-loop idiom
/// linear.
#[derive(Clone, Debug)]
pub struct MapValue {
    /// Entries in insertion order.
    entries: Vec<(MapKey, Value)>,
    /// Lookup index from key to position in `entries`.
    index: HashMap<MapKey, usize>,
}

impl MapValue {
    /// Creates a map from entries in insertion order.
    ///
    /// Returns `None` when a key repeats, which the evaluator reports as a
    /// duplicate-key error at the literal.
    pub(crate) fn from_entries(entries: Vec<(Value, Value)>) -> Option<Self> {
        let mut map = Self {
            entries: Vec::with_capacity(entries.len()),
            index: HashMap::with_capacity(entries.len()),
        };
        for (key, value) in entries {
            let key = MapKey::from_value(&key)?;
            let position = map.entries.len();
            if map.index.insert(key.clone(), position).is_some() {
                return None;
            }
            map.entries.push((key, value));
        }
        Some(map)
    }

    /// Returns the number of entries.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the map holds no entries.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Looks up a key's value.
    pub(crate) fn get(&self, key: &Value) -> Option<&Value> {
        let key = MapKey::from_value(key)?;
        self.index
            .get(&key)
            .map(|&position| &self.entries[position].1)
    }

    /// Returns whether `key` is present.
    pub(crate) fn contains_key(&self, key: &Value) -> bool {
        MapKey::from_value(key).is_some_and(|key| self.index.contains_key(&key))
    }

    /// Returns a copy of the map with `key` bound to `value`.
    ///
    /// An existing key keeps its insertion position and gains the new value;
    /// a fresh key is appended. Returns false when the key is not a legal
    /// map key, leaving the map unchanged.
    pub(crate) fn insert_in(&mut self, key: &Value, value: Value) -> bool {
        let Some(key) = MapKey::from_value(key) else {
            return false;
        };
        match self.index.get(&key).copied() {
            Some(position) => self.entries[position].1 = value,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
            }
        }
        true
    }

    /// Removes `key` from the map. Returns false when the key is not a
    /// legal key or is absent, leaving the map unchanged.
    pub(crate) fn remove_in(&mut self, key: &Value) -> bool {
        let Some(key) = MapKey::from_value(key) else {
            return false;
        };
        let Some(position) = self.index.remove(&key) else {
            return false;
        };
        self.entries.remove(position);
        for entry_position in self.index.values_mut() {
            if *entry_position > position {
                *entry_position -= 1;
            }
        }
        true
    }

    /// Iterates over the keys in insertion order.
    pub(crate) fn keys(&self) -> impl Iterator<Item = Value> + '_ {
        self.entries.iter().map(|(key, _)| key.to_value())
    }

    /// Iterates over the values in insertion order.
    pub(crate) fn values(&self) -> impl Iterator<Item = Value> + '_ {
        self.entries.iter().map(|(_, value)| value.clone())
    }
}

/// Maps are equal when they hold the same keys mapped to equal values,
/// regardless of insertion order.
impl PartialEq for MapValue {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        self.entries.iter().all(|(key, value)| {
            other
                .get(&key.to_value())
                .is_some_and(|other| other == value)
        })
    }
}

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
    /// An immutable map from string or integer keys to values.
    ///
    /// Entries live behind an `Rc` for the same sharing rules as
    /// [`Value::List`]; iteration and rendering follow insertion order and
    /// equality ignores it. Keys are inspected only through [`MapValue`]'s
    /// internal representation.
    Map(Rc<MapValue>),
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
            Value::Map(_) => "map",
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
            Value::Map(map) => {
                if map.is_empty() {
                    return "#{}".to_owned();
                }
                let rendered = map
                    .entries
                    .iter()
                    .map(|(key, value)| {
                        format!(
                            "{}: {}",
                            key.to_value().element_text(),
                            value.element_text()
                        )
                    })
                    .collect::<Vec<String>>()
                    .join(", ");
                format!("#{{{rendered}}}")
            }
        }
    }

    /// Renders the value the way it appears *inside* an echoed list.
    ///
    /// Strings are quoted so list contents stay unambiguous; everything
    /// else renders exactly as it does at top level. Diagnostics reuse this
    /// rendering for map keys.
    pub(crate) fn element_text(&self) -> String {
        match self {
            Value::Str(string) => format!("\"{string}\""),
            other => other.display_text(),
        }
    }
}
