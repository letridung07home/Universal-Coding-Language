//! Parser: builds an abstract syntax tree from tokens.

use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::lexer::{Keyword, Token, TokenKind};
use crate::source::Span;

/// A node in the abstract syntax tree.
///
/// Each node carries a [`Span`] indicating its source location and an
/// [`AstKind`] describing the syntactic construct it represents.
#[derive(Clone, Debug, PartialEq)]
pub struct AstNode {
    /// The byte span of this node in the source file.
    pub span: Span,
    /// The syntactic construct represented by this node.
    pub kind: AstKind,
}

impl AstNode {
    /// Creates a new AST node with the given span and kind.
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
    /// `let` is a reserved keyword token, so the declaration is recognized
    /// from that keyword rather than from the token shape.
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
///
/// The parser uses recursive descent with operator precedence parsing
/// to handle the language's grammar.
pub struct Parser {
    /// The token stream to parse.
    tokens: Vec<Token>,
    /// The current position in the token stream.
    cursor: usize,
    /// The current expression-nesting depth, used to guard against deeply
    /// nested input that would otherwise overflow the call stack.
    depth: usize,
}

impl Parser {
    /// Creates a parser ready to consume the given `tokens`.
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            cursor: 0,
            depth: 0,
        }
    }

    /// Parses the token stream into an abstract syntax tree.
    ///
    /// Returns `Some(AstNode)` containing the parsed program, or `None` if
    /// parsing failed completely.
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

    /// Parses a single statement from the token stream.
    ///
    /// A statement can be a declaration (`let x = 5;`), an assignment (`x = 5;`),
    /// an expression statement (`x + 5;`), or a block (`{ ... }`).
    fn parse_statement(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        // `let` is a reserved keyword, so `let name = value` is recognized
        // from its keyword token rather than from the token shape.
        if self.check_keyword(Keyword::Let) {
            let keyword = self.advance().span;

            if !self.check_kind(TokenKind::Ident) {
                self.error_current("expected an identifier after `let`", sink);
                return None;
            }
            let name = self.advance().span;

            if !self.consume_punctuation('=') {
                self.error_current("expected `=` after the declared identifier", sink);
                return None;
            }

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

    /// Parses an expression with operator precedence parsing.
    ///
    /// Uses the Pratt parsing algorithm to handle operator precedence and
    /// associativity correctly. The `minimum_precedence` parameter ensures
    /// that operators with lower precedence are not parsed in this context.
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

    /// Parses a unary operator expression (e.g., `-x`, `!true`).
    fn parse_unary(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        // Track expression-nesting depth so pathologically nested input is
        // rejected with an error instead of overflowing the call stack.
        self.depth += 1;
        if self.depth > MAX_NESTING_DEPTH {
            self.depth -= 1;
            self.error_current("expression nesting is too deep", sink);
            return None;
        }

        let result = match self.prefix_operator() {
            Some(operator) => {
                let start = self.advance().span.start;
                match self.parse_unary(sink) {
                    Some(operand) => Some(AstNode::new(
                        Span::new(start, operand.span.end),
                        AstKind::Unary {
                            operator,
                            operand: Box::new(operand),
                        },
                    )),
                    None => None,
                }
            }
            None => self.parse_primary(sink),
        };

        self.depth -= 1;
        result
    }

    /// Parses a primary expression (the atomic elements of expressions).
    ///
    /// A primary can be a literal, an identifier, a parenthesized expression,
    /// or a block.
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

    /// Parses a block statement (`{ ... }`).
    ///
    /// A block introduces a new lexical scope and contains zero or more statements.
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

    /// Returns the infix operator at the current position, if any.
    fn infix_operator(&self) -> Option<char> {
        match self.tokens.get(self.cursor).map(|token| token.kind) {
            Some(TokenKind::Punctuation(operator)) if precedence(operator).is_some() => {
                Some(operator)
            }
            _ => None,
        }
    }

    /// Returns the prefix operator at the current position, if any.
    fn prefix_operator(&self) -> Option<char> {
        match self.tokens.get(self.cursor).map(|token| token.kind) {
            Some(TokenKind::Punctuation(operator)) if matches!(operator, '+' | '-' | '!') => {
                Some(operator)
            }
            _ => None,
        }
    }

    /// Advances the cursor past the current statement for error recovery.
    ///
    /// This skips tokens until a statement terminator (`;` or `}`) is found,
    /// allowing parsing to continue after a syntax error.
    fn recover_statement(&mut self) {
        while !self.at_eof() && !self.check_punctuation(';') && !self.check_punctuation('}') {
            self.advance();
        }
    }

    /// Emits an error diagnostic at the current token position.
    fn error_current(&self, message: &str, sink: &mut DiagnosticSink) {
        sink.emit(Diagnostic::error(message).at(self.current_span()));
    }

    /// Returns the span of the current token, or a zero span if at EOF.
    fn current_span(&self) -> Span {
        self.tokens
            .get(self.cursor)
            .or_else(|| self.tokens.last())
            .map_or(Span::new(0, 0), |token| token.span)
    }

    /// Advances the cursor and returns the current token.
    fn advance(&mut self) -> Token {
        let token = self.tokens[self.cursor];
        self.cursor += 1;
        token
    }

    /// Returns true if the cursor is at the end of the token stream.
    fn at_eof(&self) -> bool {
        matches!(
            self.tokens.get(self.cursor).map(|token| token.kind),
            Some(TokenKind::Eof) | None
        )
    }

    /// Returns true if the current token has the given kind.
    fn check_kind(&self, kind: TokenKind) -> bool {
        self.tokens
            .get(self.cursor)
            .is_some_and(|token| token.kind == kind)
    }

    /// Returns true if the current token is the given keyword.
    fn check_keyword(&self, keyword: Keyword) -> bool {
        self.tokens
            .get(self.cursor)
            .is_some_and(|token| token.kind == TokenKind::Keyword(keyword))
    }

    /// Returns true if the current token is the given punctuation character.
    fn check_punctuation(&self, punctuation: char) -> bool {
        self.tokens
            .get(self.cursor)
            .is_some_and(|token| token.kind == TokenKind::Punctuation(punctuation))
    }

    /// If the current token is the given punctuation, consumes it and returns true.
    /// Otherwise returns false without consuming.
    fn consume_punctuation(&mut self, punctuation: char) -> bool {
        if self.check_punctuation(punctuation) {
            self.advance();
            true
        } else {
            false
        }
    }
}

/// The maximum expression-nesting depth the parser will accept.
///
/// Deeply nested input (for example, thousands of nested parentheses or
/// unary operators) is rejected with an error rather than overflowing the
/// call stack during recursive-descent parsing.
const MAX_NESTING_DEPTH: usize = 256;

/// Operator precedence levels (higher = binds tighter).
///
/// These constants are used to determine the order of operations in expressions.
const PRECEDENCE_OR: u8 = 1;
const PRECEDENCE_AND: u8 = 2;
const PRECEDENCE_COMPARISON: u8 = 3;
const PRECEDENCE_ADDITIVE: u8 = 4;
const PRECEDENCE_MULTIPLICATIVE: u8 = 5;
const PRECEDENCE_EXPONENTIATION: u8 = 6;

/// Returns the precedence level of an infix operator.
///
/// Returns `None` for characters that are not valid infix operators.
/// Higher values indicate tighter binding (evaluated first).
fn precedence(operator: char) -> Option<u8> {
    match operator {
        '|' => Some(PRECEDENCE_OR),
        '&' => Some(PRECEDENCE_AND),
        '<' | '>' => Some(PRECEDENCE_COMPARISON),
        '+' | '-' => Some(PRECEDENCE_ADDITIVE),
        '*' | '/' | '%' => Some(PRECEDENCE_MULTIPLICATIVE),
        '^' => Some(PRECEDENCE_EXPONENTIATION),
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

    #[test]
    fn let_is_a_keyword_not_an_identifier() {
        let (ast, sink) = parse("let x = 1;");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        assert_eq!(statements.len(), 1);
        assert!(matches!(statements[0].kind, AstKind::Let { .. }));
    }

    #[test]
    fn an_arbitrary_identifier_cannot_declare_a_binding() {
        let (_ast, sink) = parse("foo bar = 3;");

        assert!(sink.has_errors());
    }

    #[test]
    fn let_cannot_be_used_as_an_identifier() {
        let (_ast, sink) = parse("let let = 5;");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("expected an identifier"))
        );
    }

    #[test]
    fn rejects_deeply_nested_parentheses() {
        let depth = 1000;
        let source_text = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
        let source = SourceFile::new("test.ucl", &source_text);
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);
        let ast = Parser::new(tokens).parse(&mut sink);

        assert!(ast.is_some());
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("nesting is too deep"))
        );
    }

    #[test]
    fn rejects_deeply_nested_unary_operators() {
        let depth = 1000;
        let source_text = format!("{}1", "-".repeat(depth));
        let source = SourceFile::new("test.ucl", &source_text);
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);
        let ast = Parser::new(tokens).parse(&mut sink);

        assert!(ast.is_some());
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("nesting is too deep"))
        );
    }

    #[test]
    fn reports_a_missing_identifier_after_let() {
        let (_ast, sink) = parse("let = 5;");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("after `let`"))
        );
    }

    #[test]
    fn reports_a_missing_equals_in_a_declaration() {
        let (_ast, sink) = parse("let x 5;");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("expected `=`"))
        );
    }

    #[test]
    fn reports_an_unbalanced_parenthesis() {
        let (_ast, sink) = parse("(1 + 2;");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("expected `)`"))
        );
    }

    #[test]
    fn reports_an_unbalanced_brace() {
        let (_ast, sink) = parse("{ 1 + 2;");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("expected `}`"))
        );
    }

    #[test]
    fn reports_a_missing_semicolon_between_statements() {
        let (_ast, sink) = parse("1 2;");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("expected `;`"))
        );
    }

    #[test]
    fn empty_statements_are_ignored() {
        let (_ast, sink) = parse(";;;");

        assert!(!sink.has_errors());
    }

    #[test]
    fn unary_operators_apply_to_groups() {
        let (ast, sink) = parse("-(1 + 2);");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        assert!(matches!(
            statements[0].kind,
            AstKind::Unary { operator: '-', .. }
        ));
    }
}
