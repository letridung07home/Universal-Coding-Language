//! Parser: builds an abstract syntax tree from tokens.

use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::lexer::{Token, TokenKind};
use crate::source::Span;

/// A node in the abstract syntax tree.
#[derive(Clone, Debug, PartialEq)]
pub struct AstNode {
    /// Where this node came from in the source.
    pub span: Span,
    /// The syntactic construct represented by this node.
    pub kind: AstKind,
}

impl AstNode {
    fn new(span: Span, kind: AstKind) -> Self {
        Self { span, kind }
    }
}

/// The syntactic constructs currently understood by the parser.
#[derive(Clone, Debug, PartialEq)]
pub enum AstKind {
    /// A complete source file.
    Program {
        /// The statements in source order.
        statements: Vec<AstNode>,
    },
    /// A declaration in the form `let name = value`.
    ///
    /// The lexer currently classifies keywords as identifiers, so the parser
    /// recognizes the declaration shape `Ident Ident = expression`.
    Let {
        /// Span of the declaration keyword.
        keyword: Span,
        /// Span of the declared name.
        name: Span,
        /// The initializer expression.
        value: Box<AstNode>,
    },
    /// An integer literal.
    Integer,
    /// An identifier reference.
    Identifier,
    /// A parenthesized expression.
    Group {
        /// The expression between the parentheses.
        expression: Box<AstNode>,
    },
    /// A block delimited by braces.
    Block {
        /// The statements in source order.
        statements: Vec<AstNode>,
    },
    /// A prefix operator expression.
    Unary {
        /// The operator character.
        operator: char,
        /// The operand.
        operand: Box<AstNode>,
    },
    /// An infix operator expression.
    Binary {
        /// The operator character.
        operator: char,
        /// The left operand.
        left: Box<AstNode>,
        /// The right operand.
        right: Box<AstNode>,
    },
    /// An assignment in the form `target = value`.
    Assignment {
        /// The expression being assigned to.
        target: Box<AstNode>,
        /// The assigned expression.
        value: Box<AstNode>,
    },
}

/// Builds an [`AstNode`] from a stream of [`Token`]s.
pub struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser {
    /// Creates a parser ready to consume `tokens`.
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, cursor: 0 }
    }

    /// Parses the token stream into an AST, emitting errors into `sink`.
    ///
    /// Parsing continues after malformed statements where possible, allowing
    /// callers to report more than one syntax error in a single pass.
    pub fn parse(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        if self.tokens.is_empty() {
            sink.emit(Diagnostic::error("parser received an empty token stream"));
            return None;
        }

        let start = self.tokens[0].span.start;
        let mut statements = Vec::new();

        while !self.at_eof() {
            if self.consume_punctuation(';') {
                continue;
            }

            let statement_start = self.cursor;
            match self.parse_statement(sink) {
                Some(statement) => statements.push(statement),
                None => {
                    self.recover_statement();
                    if self.cursor == statement_start && !self.at_eof() {
                        self.advance();
                    }
                }
            }

            // Semicolons terminate statements, but the final statement may
            // omit one before EOF or a closing brace.
            if !self.consume_punctuation(';') && !self.at_eof() && !self.check_punctuation('}') {
                self.error_current("expected `;` between statements", sink);
                self.recover_statement();
                self.consume_punctuation(';');
            }
        }

        let end = self.current_span().end;
        Some(AstNode::new(
            Span::new(start, end),
            AstKind::Program { statements },
        ))
    }

    fn parse_statement(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        // Until keywords have their own token kinds, `let name = value` is
        // identified by its unambiguous token shape.
        if self.check_kind(TokenKind::Ident)
            && self.peek_kind(1) == Some(TokenKind::Ident)
            && self.peek_is_punctuation(2, '=')
        {
            let keyword = self.advance().span;
            let name = self.advance().span;
            self.advance(); // `=`
            let value = self.parse_expression(0, sink)?;
            return Some(AstNode::new(
                Span::new(keyword.start, value.span.end),
                AstKind::Let {
                    keyword,
                    name,
                    value: Box::new(value),
                },
            ));
        }

        let target = self.parse_expression(0, sink)?;
        if self.consume_punctuation('=') {
            let value = self.parse_expression(0, sink)?;
            let span = Span::new(target.span.start, value.span.end);
            Some(AstNode::new(
                span,
                AstKind::Assignment {
                    target: Box::new(target),
                    value: Box::new(value),
                },
            ))
        } else {
            Some(target)
        }
    }

    fn parse_expression(
        &mut self,
        minimum_precedence: u8,
        sink: &mut DiagnosticSink,
    ) -> Option<AstNode> {
        let mut left = self.parse_unary(sink)?;

        while let Some(operator) = self.infix_operator() {
            let Some(precedence) = precedence(operator) else {
                break;
            };
            if precedence < minimum_precedence {
                break;
            }

            self.advance();
            let right = self.parse_expression(precedence + 1, sink)?;
            let span = Span::new(left.span.start, right.span.end);
            left = AstNode::new(
                span,
                AstKind::Binary {
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            );
        }

        Some(left)
    }

    fn parse_unary(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        if let Some(operator) = self.prefix_operator() {
            let start = self.advance().span.start;
            let operand = self.parse_unary(sink)?;
            return Some(AstNode::new(
                Span::new(start, operand.span.end),
                AstKind::Unary {
                    operator,
                    operand: Box::new(operand),
                },
            ));
        }

        self.parse_primary(sink)
    }

    fn parse_primary(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        let token = *self.tokens.get(self.cursor)?;
        match token.kind {
            TokenKind::Integer => {
                self.advance();
                Some(AstNode::new(token.span, AstKind::Integer))
            }
            TokenKind::Ident => {
                self.advance();
                Some(AstNode::new(token.span, AstKind::Identifier))
            }
            TokenKind::Punctuation('(') => {
                let start = self.advance().span.start;
                let expression = self.parse_expression(0, sink)?;
                let end = if self.consume_punctuation(')') {
                    self.tokens[self.cursor - 1].span.end
                } else {
                    self.error_current("expected `)`", sink);
                    expression.span.end
                };
                Some(AstNode::new(
                    Span::new(start, end),
                    AstKind::Group {
                        expression: Box::new(expression),
                    },
                ))
            }
            TokenKind::Punctuation('{') => self.parse_block(sink),
            TokenKind::Eof => {
                self.error_current("expected an expression, found end of input", sink);
                None
            }
            _ => {
                self.error_current("expected an expression", sink);
                None
            }
        }
    }

    fn parse_block(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        let start = self.advance().span.start;
        let mut statements = Vec::new();

        while !self.at_eof() && !self.check_punctuation('}') {
            if self.consume_punctuation(';') {
                continue;
            }
            let statement_start = self.cursor;
            if let Some(statement) = self.parse_statement(sink) {
                statements.push(statement);
            } else {
                self.recover_statement();
                if self.cursor == statement_start && !self.at_eof() {
                    self.advance();
                }
            }

            if !self.consume_punctuation(';') && !self.at_eof() && !self.check_punctuation('}') {
                self.error_current("expected `;` between statements", sink);
                self.recover_statement();
                self.consume_punctuation(';');
            }
        }

        if self.consume_punctuation('}') {
            let end = self.tokens[self.cursor - 1].span.end;
            Some(AstNode::new(
                Span::new(start, end),
                AstKind::Block { statements },
            ))
        } else {
            self.error_current("expected `}`", sink);
            None
        }
    }

    fn infix_operator(&self) -> Option<char> {
        match self.tokens.get(self.cursor).map(|token| token.kind) {
            Some(TokenKind::Punctuation(operator)) if precedence(operator).is_some() => {
                Some(operator)
            }
            _ => None,
        }
    }

    fn prefix_operator(&self) -> Option<char> {
        match self.tokens.get(self.cursor).map(|token| token.kind) {
            Some(TokenKind::Punctuation(operator)) if matches!(operator, '+' | '-' | '!') => {
                Some(operator)
            }
            _ => None,
        }
    }

    fn recover_statement(&mut self) {
        while !self.at_eof() && !self.check_punctuation(';') && !self.check_punctuation('}') {
            self.advance();
        }
    }

    fn error_current(&self, message: &str, sink: &mut DiagnosticSink) {
        sink.emit(Diagnostic::error(message).at(self.current_span()));
    }

    fn current_span(&self) -> Span {
        self.tokens
            .get(self.cursor)
            .or_else(|| self.tokens.last())
            .map_or(Span::new(0, 0), |token| token.span)
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.cursor];
        self.cursor += 1;
        token
    }

    fn at_eof(&self) -> bool {
        matches!(
            self.tokens.get(self.cursor).map(|token| token.kind),
            Some(TokenKind::Eof) | None
        )
    }

    fn check_kind(&self, kind: TokenKind) -> bool {
        self.tokens
            .get(self.cursor)
            .is_some_and(|token| token.kind == kind)
    }

    fn check_punctuation(&self, punctuation: char) -> bool {
        self.tokens
            .get(self.cursor)
            .is_some_and(|token| token.kind == TokenKind::Punctuation(punctuation))
    }

    fn consume_punctuation(&mut self, punctuation: char) -> bool {
        if self.check_punctuation(punctuation) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn peek_kind(&self, offset: usize) -> Option<TokenKind> {
        self.tokens
            .get(self.cursor + offset)
            .map(|token| token.kind)
    }

    fn peek_is_punctuation(&self, offset: usize, punctuation: char) -> bool {
        self.peek_kind(offset) == Some(TokenKind::Punctuation(punctuation))
    }
}

fn precedence(operator: char) -> Option<u8> {
    match operator {
        '|' => Some(1),
        '&' => Some(2),
        '<' | '>' => Some(3),
        '+' | '-' => Some(4),
        '*' | '/' | '%' => Some(5),
        '^' => Some(6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticSink;
    use crate::lexer::Lexer;
    use crate::source::SourceFile;

    fn parse(source_text: &str) -> (AstNode, DiagnosticSink) {
        let source = SourceFile::new("test.ucl", source_text);
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);
        let ast = Parser::new(tokens)
            .parse(&mut sink)
            .expect("parser should return a program");
        (ast, sink)
    }

    #[test]
    fn parses_declaration_and_operator_precedence() {
        let (ast, sink) = parse("let answer = 2 + 3 * 4;");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        assert_eq!(statements.len(), 1);
        assert!(matches!(statements[0].kind, AstKind::Let { .. }));

        let AstKind::Let { value, .. } = &statements[0].kind else {
            unreachable!()
        };
        assert!(matches!(value.kind, AstKind::Binary { operator: '+', .. }));
        let AstKind::Binary { right, .. } = &value.kind else {
            unreachable!()
        };
        assert!(matches!(right.kind, AstKind::Binary { operator: '*', .. }));
    }

    #[test]
    fn parses_blocks_and_parenthesized_expressions() {
        let (ast, sink) = parse("{ x = (1 + 2); }");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        assert!(matches!(statements[0].kind, AstKind::Block { .. }));
    }

    #[test]
    fn reports_missing_expression_and_recovers() {
        let (_ast, sink) = parse("x = ; y = 2;");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| { diagnostic.message.contains("expected an expression") })
        );
    }
}
