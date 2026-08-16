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
pub mod parser;
pub mod source;

pub use diagnostic::{Diagnostic, DiagnosticSink, Severity};
pub use evaluator::{Evaluator, Value};
pub use lexer::{Lexer, Token, TokenKind};
pub use parser::{AstNode, Parser};
pub use source::{BytePos, SourceFile, Span};
