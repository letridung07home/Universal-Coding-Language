//! Parser: builds an abstract syntax tree from tokens.

use crate::diagnostic::DiagnosticSink;
use crate::lexer::Token;
use crate::source::Span;

/// A node in the abstract syntax tree.
///
/// The exact shape of the AST will be defined alongside the language
/// specification. This placeholder stands in until then.
#[derive(Clone, Debug, PartialEq)]
pub struct AstNode {
    /// Where this node came from in the source.
    pub span: Span,
}

/// Builds an [`AstNode`] from a stream of [`Token`]s.
pub struct Parser {
    tokens: Vec<Token>,
}

impl Parser {
    /// Creates a parser ready to consume `tokens`.
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens }
    }

    /// Parses the token stream into an AST, emitting errors into `sink`.
    pub fn parse(&mut self, _sink: &mut DiagnosticSink) -> Option<AstNode> {
        // Keep the field read until the parser is implemented.
        let _ = &self.tokens;
        todo!("parser not yet implemented");
    }
}
