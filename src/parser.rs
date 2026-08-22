//! Parser: builds an abstract syntax tree from tokens.

use std::fmt;

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
    /// A boolean literal: `true` or `false`.
    BooleanLiteral(
        /// The literal's value.
        bool,
    ),
    /// A string literal, such as `"hello"`.
    ///
    /// The node's span covers the entire lexeme including the surrounding
    /// quotes; the decoded contents are recovered from the source text via
    /// [`crate::lexer::unescape_string`] at evaluation time.
    StringLiteral,
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
        /// The operator.
        operator: BinaryOperator,
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
    /// A conditional expression: `if condition { ... } else { ... }`.
    ///
    /// The value of the expression is the value of whichever branch runs;
    /// a missing `else` branch contributes
    /// [`unit`](crate::evaluator::Value::Unit) semantics.
    If {
        /// The condition; it must evaluate to a boolean.
        condition: Box<AstNode>,
        /// The block evaluated when the condition is true.
        then_branch: Box<AstNode>,
        /// The block (or nested `if`) evaluated otherwise.
        else_branch: Option<Box<AstNode>>,
    },
    /// A loop statement: `while condition { ... }`.
    While {
        /// The condition checked before each iteration; must be a boolean.
        condition: Box<AstNode>,
        /// The block evaluated once per iteration.
        body: Box<AstNode>,
    },
    /// A function declaration or literal: `fn name(parameters) { ... }` or
    /// `fn(parameters) { ... }`.
    ///
    /// A declaration binds `name` in the enclosing scope; a literal is an
    /// anonymous expression value.
    Function {
        /// Span of the declaration keyword.
        keyword: Span,
        /// Span of the declared function name, if any.
        name: Option<Span>,
        /// Spans of the parameter names in source order.
        parameters: Vec<Span>,
        /// The function body.
        body: Box<AstNode>,
    },
    /// A function call: `callee(argument, ...)`.
    Call {
        /// The expression that evaluates to the callable function.
        callee: Box<AstNode>,
        /// Arguments in source order.
        arguments: Vec<AstNode>,
    },
    /// A return statement: `return expression;` or `return;`.
    Return {
        /// The returned expression, if any. A bare `return` returns unit.
        value: Option<Box<AstNode>>,
    },
    /// An import statement: `use "path.ucl";`.
    ///
    /// The path span covers the string literal; the decoded text is
    /// recovered from the source via [`crate::lexer::unescape_string`] at
    /// evaluation time. Only valid at the top level of a program.
    Use {
        /// Span of the `use` keyword.
        keyword: Span,
        /// Span of the module path string literal.
        path: Span,
    },
}

/// An infix binary operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOperator {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
    /// `^` (exponentiation)
    Pow,
    /// `<`
    Less,
    /// `>`
    Greater,
    /// `<=`
    LessEqual,
    /// `>=`
    GreaterEqual,
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `&` (logical and)
    And,
    /// `|` (logical or)
    Or,
}

impl fmt::Display for BinaryOperator {
    /// Formats the operator as its source symbol.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let symbol = match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
            Self::Pow => "^",
            Self::Less => "<",
            Self::Greater => ">",
            Self::LessEqual => "<=",
            Self::GreaterEqual => ">=",
            Self::Equal => "==",
            Self::NotEqual => "!=",
            Self::And => "&",
            Self::Or => "|",
        };
        f.write_str(symbol)
    }
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
    /// Whether the last parse stopped because the input ended in the middle
    /// of a construct (an unbalanced brace, a dangling operator, and so on).
    /// Interactive front ends use this to keep reading continuation lines
    /// instead of reporting an error.
    incomplete: bool,
}

impl Parser {
    /// Creates a parser ready to consume the given `tokens`.
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            cursor: 0,
            depth: 0,
            incomplete: false,
        }
    }

    /// Returns true if the last parse ended because the input ran out in the
    /// middle of a construct rather than because of genuinely malformed
    /// syntax. An error anchored at end-of-input almost always means more
    /// text was expected; an error at any real token means the input is
    /// wrong as written.
    pub fn is_incomplete(&self) -> bool {
        self.incomplete
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
            // `use` is only valid at the top level of a program, so it is
            // recognized here rather than in `parse_statement`, where it
            // would also be accepted inside blocks and function bodies.
            if self.check_keyword(Keyword::Use) {
                match self.parse_use(sink) {
                    Some(statement) => statements.push(statement),
                    None => {
                        self.recover_statement();
                        if self.cursor == statement_start && !self.at_eof() {
                            self.advance();
                        }
                    }
                }
            } else {
                match self.parse_statement(sink) {
                    Some(statement) => statements.push(statement),
                    None => {
                        self.recover_statement();
                        if self.cursor == statement_start && !self.at_eof() {
                            self.advance();
                        }
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

    /// Parses an import statement: `use "path";`.
    fn parse_use(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        let keyword = self.advance().span;

        if !self.check_kind(TokenKind::StringLiteral) {
            self.error_current("expected a module path string after `use`", sink);
            return None;
        }
        let path = self.advance().span;

        Some(AstNode::new(
            Span::new(keyword.start, path.end),
            AstKind::Use { keyword, path },
        ))
    }

    /// Parses a single statement from the token stream.
    ///
    /// A statement can be a declaration (`let x = 5;`), a function declaration
    /// (`fn add(left, right) { left + right; }`), an assignment (`x = 5;`), an
    /// expression statement (`x + 5;`), or a block (`{ ... }`).
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

        // A named declaration (`fn name(...)`) is a statement; an anonymous
        // literal (`fn(...)`) falls through to expression parsing so call
        // syntax like `fn(x) { x; }(1);` works.
        if self.check_keyword(Keyword::Function) && self.peek_is_ident() {
            return self.parse_function(sink);
        }

        // `return` is a statement, not an expression.
        if self.check_keyword(Keyword::Return) {
            let keyword = self.advance().span;
            let value = if self.check_punctuation(';') || self.at_eof() {
                None
            } else {
                Some(Box::new(self.parse_expression(0, sink)?))
            };
            let end = value.as_ref().map_or(keyword.end, |value| value.span.end);
            return Some(AstNode::new(
                Span::new(keyword.start, end),
                AstKind::Return { value },
            ));
        }

        // A `while` loop is a statement, not an expression: it evaluates to
        // unit and may not appear inside a larger expression.
        if self.check_keyword(Keyword::While) {
            return self.parse_while(sink);
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
            None => self.parse_postfix(sink),
        };

        self.depth -= 1;
        result
    }

    /// Parses a postfix expression, including one or more function calls.
    ///
    /// Calls bind tighter than unary and infix operators, so `-f(1)` parses as
    /// `-(f(1))` and `f(1)(2)` is a nested call expression.
    fn parse_postfix(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        let mut callee = self.parse_primary(sink)?;

        while self.consume_punctuation('(') {
            let mut arguments = Vec::new();
            if !self.check_punctuation(')') {
                loop {
                    arguments.push(self.parse_expression(0, sink)?);
                    if self.consume_punctuation(',') {
                        continue;
                    }
                    break;
                }
            }

            let end = if self.consume_punctuation(')') {
                self.tokens[self.cursor - 1].span.end
            } else {
                self.error_current("expected `)` after function arguments", sink);
                arguments
                    .last()
                    .map_or(callee.span.end, |argument| argument.span.end)
            };
            let span = Span::new(callee.span.start, end);
            callee = AstNode::new(
                span,
                AstKind::Call {
                    callee: Box::new(callee),
                    arguments,
                },
            );
        }

        Some(callee)
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
            TokenKind::Keyword(Keyword::True | Keyword::False) => {
                self.advance();
                let value = token.kind == TokenKind::Keyword(Keyword::True);
                Some(AstNode::new(token.span, AstKind::BooleanLiteral(value)))
            }
            TokenKind::Keyword(Keyword::If) => self.parse_if(sink),
            // An anonymous function literal: `fn(x) { ... }`. Named
            // declarations are handled at statement level.
            TokenKind::Keyword(Keyword::Function) if self.peek_is_open_paren() => {
                self.parse_function(sink)
            }
            TokenKind::StringLiteral => {
                self.advance();
                Some(AstNode::new(token.span, AstKind::StringLiteral))
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

    /// Parses an `if` expression: `if condition { ... } [else { ... | if ... }]`.
    ///
    /// Parentheses around the condition are optional. The `else` branch may
    /// be a block or a nested `if`, allowing `else if` chains.
    fn parse_if(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        // Each nested `else if` adds a stack frame that the expression
        // nesting counter would not see, so track depth here as well to
        // bound pathological chains.
        self.depth += 1;
        if self.depth > MAX_NESTING_DEPTH {
            self.depth -= 1;
            self.error_current("expression nesting is too deep", sink);
            return None;
        }

        let result = self.parse_if_inner(sink);
        self.depth -= 1;
        result
    }

    /// The body of [`Parser::parse_if`], run under its depth guard.
    fn parse_if_inner(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        let start = self.advance().span.start;

        let condition = self.parse_expression(0, sink)?;
        let then_branch = self.parse_block(sink)?;

        let else_branch = if self.check_keyword(Keyword::Else) {
            self.advance();
            let branch = if self.check_keyword(Keyword::If) {
                self.parse_if(sink)?
            } else {
                self.parse_block(sink)?
            };
            Some(Box::new(branch))
        } else {
            None
        };

        let end = else_branch
            .as_ref()
            .map_or(then_branch.span.end, |branch| branch.span.end);
        Some(AstNode::new(
            Span::new(start, end),
            AstKind::If {
                condition: Box::new(condition),
                then_branch: Box::new(then_branch),
                else_branch,
            },
        ))
    }

    /// Parses a named function declaration: `fn name(parameters) { ... }`.
    fn parse_function(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        let keyword = self.advance().span;
        // An identifier makes this a named declaration; its absence starts
        // an anonymous function literal (`fn(x) { ... }`).
        let name = if self.check_kind(TokenKind::Ident) {
            Some(self.advance().span)
        } else {
            None
        };

        if !self.consume_punctuation('(') {
            self.error_current("expected `(` after `fn`", sink);
            return None;
        }

        let mut parameters = Vec::new();
        if !self.check_punctuation(')') {
            loop {
                if !self.check_kind(TokenKind::Ident) {
                    self.error_current("expected an identifier in the parameter list", sink);
                    return None;
                }
                parameters.push(self.advance().span);
                if self.consume_punctuation(',') {
                    continue;
                }
                break;
            }
        }

        if !self.consume_punctuation(')') {
            self.error_current("expected `)` after function parameters", sink);
            return None;
        }

        let body = self.parse_block(sink)?;
        Some(AstNode::new(
            Span::new(keyword.start, body.span.end),
            AstKind::Function {
                keyword,
                name,
                parameters,
                body: Box::new(body),
            },
        ))
    }

    /// Parses a `while` statement: `while condition { ... }`.
    ///
    /// Parentheses around the condition are optional.
    fn parse_while(&mut self, sink: &mut DiagnosticSink) -> Option<AstNode> {
        let start = self.advance().span.start;

        let condition = self.parse_expression(0, sink)?;
        let body = self.parse_block(sink)?;

        Some(AstNode::new(
            Span::new(start, body.span.end),
            AstKind::While {
                condition: Box::new(condition),
                body: Box::new(body),
            },
        ))
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

    /// Returns true if the token after the current one is `(`.
    ///
    /// Used to distinguish a function literal (`fn(`) from a named
    /// declaration (`fn name(`) in expression position.
    fn peek_is_open_paren(&self) -> bool {
        self.tokens
            .get(self.cursor + 1)
            .is_some_and(|token| token.kind == TokenKind::Punctuation('('))
    }

    /// Returns true if the token after the current one is an identifier.
    fn peek_is_ident(&self) -> bool {
        self.tokens
            .get(self.cursor + 1)
            .is_some_and(|token| token.kind == TokenKind::Ident)
    }

    /// Returns the infix operator at the current position, if any.
    fn infix_operator(&self) -> Option<BinaryOperator> {
        match self.tokens.get(self.cursor).map(|token| token.kind) {
            Some(TokenKind::Punctuation(operator)) => match operator {
                '|' => Some(BinaryOperator::Or),
                '&' => Some(BinaryOperator::And),
                '<' => Some(BinaryOperator::Less),
                '>' => Some(BinaryOperator::Greater),
                '+' => Some(BinaryOperator::Add),
                '-' => Some(BinaryOperator::Sub),
                '*' => Some(BinaryOperator::Mul),
                '/' => Some(BinaryOperator::Div),
                '%' => Some(BinaryOperator::Rem),
                '^' => Some(BinaryOperator::Pow),
                _ => None,
            },
            Some(TokenKind::LessEqual) => Some(BinaryOperator::LessEqual),
            Some(TokenKind::GreaterEqual) => Some(BinaryOperator::GreaterEqual),
            Some(TokenKind::EqualEqual) => Some(BinaryOperator::Equal),
            Some(TokenKind::NotEqual) => Some(BinaryOperator::NotEqual),
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
    ///
    /// When the offending position is end-of-input the parse is also marked
    /// incomplete; see [`Parser::is_incomplete`].
    fn error_current(&mut self, message: &str, sink: &mut DiagnosticSink) {
        if self.at_eof() {
            self.incomplete = true;
        }
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
const PRECEDENCE_EQUALITY: u8 = 3;
const PRECEDENCE_RELATIONAL: u8 = 4;
const PRECEDENCE_ADDITIVE: u8 = 5;
const PRECEDENCE_MULTIPLICATIVE: u8 = 6;
const PRECEDENCE_EXPONENTIATION: u8 = 7;

/// Returns the precedence level of an infix operator.
///
/// Higher values indicate tighter binding (evaluated first). All operators
/// are left-associative.
fn precedence(operator: BinaryOperator) -> Option<u8> {
    match operator {
        BinaryOperator::Or => Some(PRECEDENCE_OR),
        BinaryOperator::And => Some(PRECEDENCE_AND),
        BinaryOperator::Equal | BinaryOperator::NotEqual => Some(PRECEDENCE_EQUALITY),
        BinaryOperator::Less
        | BinaryOperator::Greater
        | BinaryOperator::LessEqual
        | BinaryOperator::GreaterEqual => Some(PRECEDENCE_RELATIONAL),
        BinaryOperator::Add | BinaryOperator::Sub => Some(PRECEDENCE_ADDITIVE),
        BinaryOperator::Mul | BinaryOperator::Div | BinaryOperator::Rem => {
            Some(PRECEDENCE_MULTIPLICATIVE)
        }
        BinaryOperator::Pow => Some(PRECEDENCE_EXPONENTIATION),
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
        assert!(matches!(
            value.kind,
            AstKind::Binary {
                operator: BinaryOperator::Add,
                ..
            }
        ));
        let AstKind::Binary { right, .. } = &value.kind else {
            unreachable!()
        };
        assert!(matches!(
            right.kind,
            AstKind::Binary {
                operator: BinaryOperator::Mul,
                ..
            }
        ));
    }

    #[test]
    fn parses_equality_looser_than_relational_and_tighter_than_and() {
        // `a < b == c & d` must group as `(a < b == c) & d`: relational binds
        // tighter than equality, which binds tighter than logical and.
        let (ast, sink) = parse("a < b == c & d;");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        let AstKind::Binary { operator, left, .. } = &statements[0].kind else {
            panic!("expected binary expression")
        };
        assert_eq!(*operator, BinaryOperator::And);
        let AstKind::Binary { operator, .. } = &left.kind else {
            panic!("expected binary expression")
        };
        assert_eq!(*operator, BinaryOperator::Equal);
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
    fn parses_boolean_literals() {
        let (ast, sink) = parse("true; false;");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        assert_eq!(statements[0].kind, AstKind::BooleanLiteral(true));
        assert_eq!(statements[1].kind, AstKind::BooleanLiteral(false));
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

    #[test]
    fn parses_string_literals() {
        let (ast, sink) = parse("\"hello\";");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        assert_eq!(statements[0].kind, AstKind::StringLiteral);
    }

    #[test]
    fn parses_if_with_optional_parentheses_and_else() {
        for source in ["if x { 1; } else { 2; };", "if (x) { 1; } else { 2; };"] {
            let (ast, sink) = parse(source);

            assert!(!sink.has_errors(), "for `{source}`");
            let AstKind::Program { statements } = ast.kind else {
                panic!("expected program")
            };
            let AstKind::If {
                condition,
                then_branch,
                else_branch: Some(else_branch),
            } = &statements[0].kind
            else {
                panic!("expected an if expression for `{source}`")
            };
            assert!(
                matches!(condition.kind, AstKind::Identifier | AstKind::Group { .. }),
                "for `{source}`"
            );
            assert!(matches!(then_branch.kind, AstKind::Block { .. }));
            assert!(matches!(else_branch.kind, AstKind::Block { .. }));
        }
    }

    #[test]
    fn an_if_without_else_parses_with_no_else_branch() {
        let (ast, sink) = parse("if x > 0 { x; };");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        let AstKind::If { else_branch, .. } = &statements[0].kind else {
            panic!("expected an if expression")
        };
        assert!(else_branch.is_none());
    }

    #[test]
    fn parses_else_if_chains_as_nested_conditionals() {
        let (ast, sink) = parse("if a { 1; } else if b { 2; } else { 3; };");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        let AstKind::If {
            else_branch: Some(outer),
            ..
        } = &statements[0].kind
        else {
            panic!("expected an if expression")
        };
        assert!(
            matches!(outer.kind, AstKind::If { .. }),
            "an `else if` must nest a conditional"
        );
    }

    #[test]
    fn parses_while_statements() {
        let (ast, sink) = parse("while x < 10 { x = x + 1; };");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        let AstKind::While { condition, body } = &statements[0].kind else {
            panic!("expected a while statement")
        };
        assert!(matches!(
            condition.kind,
            AstKind::Binary {
                operator: BinaryOperator::Less,
                ..
            }
        ));
        assert!(matches!(body.kind, AstKind::Block { .. }));
    }

    #[test]
    fn while_is_not_an_expression_operand() {
        let (_ast, sink) = parse("1 + while x { 2; };");

        assert!(sink.has_errors());
    }

    #[test]
    fn parses_function_declarations_and_chained_calls() {
        let (ast, sink) = parse("fn add(left, right) { left + right; }; add(20, 22);");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program");
        };
        assert!(matches!(
            statements[0].kind,
            AstKind::Function { ref parameters, .. } if parameters.len() == 2
        ));
        assert!(matches!(statements[1].kind, AstKind::Call { .. }));
    }

    #[test]
    fn calls_bind_tighter_than_unary_operators() {
        let (ast, sink) = parse("-negate(1);");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program");
        };
        let AstKind::Unary { operand, .. } = &statements[0].kind else {
            panic!("expected unary expression");
        };
        assert!(matches!(operand.kind, AstKind::Call { .. }));
    }

    #[test]
    fn reports_a_missing_block_after_if() {
        let (_ast, sink) = parse("if x 1;");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("expected `}`"))
        );
    }

    #[test]
    fn parses_function_literals_in_expression_position() {
        for source in ["let f = fn(x) { x; };", "fn(x) { x; };"] {
            let (ast, sink) = parse(source);

            assert!(!sink.has_errors(), "for `{source}`");
            let AstKind::Program { statements } = ast.kind else {
                panic!("expected program")
            };
            let function = match &statements[0].kind {
                AstKind::Let { value, .. } => value.as_ref(),
                other => match other {
                    AstKind::Function { .. } => &statements[0],
                    _ => panic!("unexpected statement for `{source}`"),
                },
            };
            let AstKind::Function { name: None, .. } = &function.kind else {
                panic!("expected an anonymous literal for `{source}`")
            };
        }
    }

    #[test]
    fn parses_calls_of_function_literals() {
        let (ast, sink) = parse("fn(a) { a; }(1);");

        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        assert!(matches!(statements[0].kind, AstKind::Call { .. }));
    }

    #[test]
    fn parses_return_statements() {
        let (ast, sink) = parse("return 1;");
        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        let AstKind::Return { value: Some(value) } = &statements[0].kind else {
            panic!("expected a return with a value")
        };
        assert!(matches!(value.kind, AstKind::Integer));

        let (ast, sink) = parse("return;");
        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        assert!(matches!(
            statements[0].kind,
            AstKind::Return { value: None }
        ));
    }

    #[test]
    fn return_binds_to_the_following_expression() {
        // `return 1 + 2;` returns the whole expression, not just `1`.
        let (ast, sink) = parse("return 1 + 2 * 3;");
        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        let AstKind::Return { value: Some(value) } = &statements[0].kind else {
            panic!("expected a return with a value")
        };
        assert!(matches!(
            value.kind,
            AstKind::Binary {
                operator: BinaryOperator::Add,
                ..
            }
        ));
    }

    #[test]
    fn input_that_ends_mid_construct_is_incomplete() {
        for source_text in [
            "let x = ",
            "1 + ",
            "fn f() {",
            "if true { 1; } else ",
            "fn(",
        ] {
            let source = SourceFile::new("repl.ucl", source_text);
            let mut sink = DiagnosticSink::new();
            let tokens = Lexer::new(&source).tokenize(&mut sink);
            assert!(!sink.has_errors(), "lexing `{source_text}`");
            let parser_tokens = tokens.clone();
            let mut parser = Parser::new(tokens);
            parser.parse(&mut sink);
            assert!(sink.has_errors(), "`{source_text}` should report an error");
            assert!(
                parser.is_incomplete(),
                "`{source_text}` should be incomplete"
            );
            let _ = parser_tokens;
        }
    }

    #[test]
    fn genuinely_wrong_input_is_not_marked_incomplete() {
        for source_text in ["1 + ) ;", "let 5 = 3;", "fn f(,) {}"] {
            let source = SourceFile::new("repl.ucl", source_text);
            let mut sink = DiagnosticSink::new();
            let tokens = Lexer::new(&source).tokenize(&mut sink);
            let mut parser = Parser::new(tokens);
            parser.parse(&mut sink);
            assert!(sink.has_errors(), "`{source_text}` should report an error");
            assert!(
                !parser.is_incomplete(),
                "`{source_text}` should be complete-but-wrong"
            );
        }
    }

    #[test]
    fn complete_input_never_reports_incomplete() {
        for source_text in ["1 + 2;", "let x = 5;", "fn f() { return 1; };"] {
            let source = SourceFile::new("repl.ucl", source_text);
            let mut sink = DiagnosticSink::new();
            let tokens = Lexer::new(&source).tokenize(&mut sink);
            let mut parser = Parser::new(tokens);
            parser.parse(&mut sink);
            assert!(!sink.has_errors(), "for `{source_text}`");
            assert!(!parser.is_incomplete(), "for `{source_text}`");
        }
    }

    #[test]
    fn parses_use_statements_with_string_paths() {
        let (ast, sink) = parse("use \"lib/math.ucl\";");
        assert!(!sink.has_errors());
        let AstKind::Program { statements } = ast.kind else {
            panic!("expected program")
        };
        assert!(matches!(statements[0].kind, AstKind::Use { .. }));
    }

    #[test]
    fn use_requires_a_string_path() {
        for source_text in ["use math;", "use;", "use \"a\" + \"b\";"] {
            let source = SourceFile::new("t.ucl", source_text);
            let mut sink = DiagnosticSink::new();
            let tokens = Lexer::new(&source).tokenize(&mut sink);
            Parser::new(tokens).parse(&mut sink);
            assert!(sink.has_errors(), "`{source_text}` should be rejected");
        }
    }

    #[test]
    fn use_is_rejected_inside_blocks_and_functions() {
        for source_text in ["{ use \"a.ucl\"; }", "fn f() { use \"a.ucl\"; };"] {
            let source = SourceFile::new("t.ucl", source_text);
            let mut sink = DiagnosticSink::new();
            let tokens = Lexer::new(&source).tokenize(&mut sink);
            Parser::new(tokens).parse(&mut sink);
            assert!(sink.has_errors(), "`{source_text}` should be rejected");
        }
    }
}
