//! Lexer: turns source text into a stream of tokens.

use crate::diagnostic::DiagnosticSink;
use crate::source::{SourceFile, Span};

/// The kind of a lexical token.
///
/// This vocabulary is intentionally small for now and will grow as the
/// language specification is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// An identifier or keyword: `foo`, `let`, `if`, ...
    Ident,
    /// An integer literal: `42`.
    Integer,
    /// A punctuation character: `(`, `)`, `{`, `}`, ...
    Punctuation(char),
    /// End of input.
    Eof,
}

/// A token produced by the [`Lexer`], tagged with its source [`Span`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// The kind of token.
    pub kind: TokenKind,
    /// Where in the source this token appears.
    pub span: Span,
}

/// Converts source text into a vector of [`Token`]s.
pub struct Lexer<'src> {
    source: &'src SourceFile,
}

impl<'src> Lexer<'src> {
    /// Creates a lexer over the given source file.
    pub fn new(source: &'src SourceFile) -> Self {
        Self { source }
    }

    /// Runs the lexer, emitting any errors into `sink`.
    pub fn tokenize(&self, _sink: &mut DiagnosticSink) -> Vec<Token> {
        // Keep the field read until the scanner is implemented.
        let _ = self.source;
        todo!("lexer not yet implemented");
    }
}
