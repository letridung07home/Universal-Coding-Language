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
pub mod fmt;
pub mod lexer;
pub mod module;
pub mod parser;
pub mod source;

pub use diagnostic::{Diagnostic, DiagnosticSink, Severity};
pub use evaluator::{
    BuiltinFunction, Environment, Evaluator, ModuleValue, Type, TypeContext, Value,
};
pub use lexer::{Keyword, Lexer, Token, TokenKind, TypeName};
pub use parser::{AstKind, AstNode, BinaryOperator, Parameter, Parser, TypeAnnotation};
pub use source::{BytePos, SourceFile, Span};
