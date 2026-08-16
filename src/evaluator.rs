//! Evaluator: executes an abstract syntax tree.

use crate::diagnostic::DiagnosticSink;
use crate::parser::AstNode;

/// A runtime value produced by evaluating a program.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// No value (unit).
    Unit,
    // TODO: integers, strings, functions, and the rest of the type system.
}

/// Walks an [`AstNode`] and computes a [`Value`].
pub struct Evaluator;

impl Evaluator {
    /// Creates an evaluator.
    pub fn new() -> Self {
        Self
    }

    /// Evaluates `root` to a value, emitting errors into `sink`.
    pub fn evaluate(&self, _root: &AstNode, _sink: &mut DiagnosticSink) -> Value {
        todo!("evaluator not yet implemented");
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}
