//! # Universal Coding Language (UCL)
//!
//! A universal coding language that excels in every task possible.
//!
//! The compiler pipeline runs in stages, each living in its own module:
//!
//! ```text
//! source text ──► lexer ──► parser ──► evaluator
//! ```

pub mod diagnostic;
pub mod evaluator;
pub mod lexer;
mod module;
pub mod parser;
pub mod source;

pub use diagnostic::{Diagnostic, DiagnosticSink, Severity};
pub use evaluator::{BuiltinFunction, Environment, Evaluator, ModuleValue, Value};
pub use lexer::{Keyword, Lexer, Token, TokenKind};
pub use parser::{AstKind, AstNode, BinaryOperator, Parser};
pub use source::{BytePos, SourceFile, Span};
