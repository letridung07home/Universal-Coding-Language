//! Lexer: turns source text into a stream of tokens.

use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::source::{SourceFile, Span};

/// A type name recognized only in an annotation position.
///
/// Type names remain ordinary identifiers outside a `:` annotation, preserving
/// the v1 reserved-keyword set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeName {
    /// The signed 64-bit integer type, written `int`.
    Integer,
    /// The boolean type, written `bool`.
    Boolean,
    /// The UTF-8 string type, written `string`.
    String,
    /// The immutable list type, written `list`.
    List,
    /// The callable function type, written `function`.
    Function,
    /// The unit type, written `unit`.
    Unit,
    /// The imported module namespace type, written `module`.
    Module,
}

impl TypeName {
    /// Recognizes a type name from its source spelling.
    fn parse(text: &str) -> Option<Self> {
        match text {
            "int" => Some(Self::Integer),
            "bool" => Some(Self::Boolean),
            "string" => Some(Self::String),
            "list" => Some(Self::List),
            "function" => Some(Self::Function),
            "unit" => Some(Self::Unit),
            "module" => Some(Self::Module),
            _ => None,
        }
    }
}

/// A reserved word recognized by the lexer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Keyword {
    /// The `let` declaration keyword.
    Let,
    /// The `true` boolean literal.
    True,
    /// The `false` boolean literal.
    False,
    /// The `if` conditional keyword.
    If,
    /// The `else` conditional keyword.
    Else,
    /// The `while` loop keyword.
    While,
    /// The `for` loop keyword.
    For,
    /// The `in` loop-header keyword.
    In,
    /// The `fn` function-declaration keyword.
    Function,
    /// The `return` statement keyword.
    Return,
    /// The `break` loop-exit keyword.
    Break,
    /// The `continue` loop-skip keyword.
    Continue,
    /// The `use` module-import keyword.
    Use,
}

/// The kind of a lexical token.
///
/// This vocabulary is intentionally small for now and will grow as the
/// language specification is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// The contextual `as` token, recognized only after `use "path"`.
    ///
    /// Outside that import position, `as` remains an ordinary identifier.
    ImportAs,
    /// A type name recognized after `:` in an annotation.
    TypeName(TypeName),
    /// An identifier: `foo`, `answer`, etc.
    ///
    /// Keywords such as `let` are produced as [`TokenKind::Keyword`] rather
    /// than as identifiers, so they cannot be used as names.
    Ident,
    /// An integer literal: `42`, `123`, etc.
    Integer,
    /// A string literal: `"hello"`, including the surrounding quotes.
    ///
    /// Escape sequences inside the literal are validated here; decoding
    /// happens when the parser or evaluator reads back the lexeme via
    /// [`unescape_string`].
    StringLiteral,
    /// A punctuation character: `(`, `)`, `{`, `}`, `+`, `-`, etc.
    Punctuation(char),
    /// A two-character operator: `<=`.
    LessEqual,
    /// A two-character operator: `>=`.
    GreaterEqual,
    /// A two-character operator: `==`.
    EqualEqual,
    /// A two-character operator: `!=`.
    NotEqual,
    /// The two-dot range separator: `..`, used in `for` headers.
    DotDot,
    /// A reserved word such as `let`.
    Keyword(Keyword),
    /// End of input marker.
    ///
    /// Always appears as the last token in the stream.
    Eof,
}

/// A token produced by the [`Lexer`], tagged with its source [`Span`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Token {
    /// The kind of token (identifier, integer, punctuation, etc.).
    pub kind: TokenKind,
    /// The byte span of this token in the source file.
    pub span: Span,
}

/// A comment encountered during tokenization.
///
/// Comments produce no tokens, but the formatter needs to know where they
/// appeared so it can preserve them; the span is recovered from the source
/// text when rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentKind {
    /// A `//` line comment, ending before its newline.
    Line,
    /// A `/* ... */` block comment, which may span lines and nest.
    Block,
}

/// A comment's kind together with its source span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommentTrivia {
    /// Whether the comment is a line or block comment.
    pub kind: CommentKind,
    /// The byte span covering the entire comment text.
    pub span: Span,
}

/// Converts source text into a vector of [`Token`]s.
///
/// The lexer performs lexical analysis, breaking source text into tokens
/// that the parser can use to build an abstract syntax tree.
pub struct Lexer<'src> {
    /// The source file being tokenized.
    source: &'src SourceFile,
}

/// Tracks the small amount of lexical context needed to recognize the
/// import-only `as` token without reserving it globally.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportContext {
    None,
    AwaitPath,
    AwaitAs,
    AwaitAlias,
}

impl<'src> Lexer<'src> {
    /// Creates a lexer over the given source file.
    pub fn new(source: &'src SourceFile) -> Self {
        Self { source }
    }

    /// Tokenizes the source file into a vector of tokens.
    ///
    /// The returned vector always ends with a [`TokenKind::Eof`] token whose
    /// span points one byte past the last byte of the source. On an unknown
    /// character, an error diagnostic is emitted and scanning continues
    /// to allow reporting multiple lexical errors.
    pub fn tokenize(&self, sink: &mut DiagnosticSink) -> Vec<Token> {
        self.tokenize_with_comments(sink).0
    }

    /// Tokenizes like [`Lexer::tokenize`] and also reports every comment
    /// encountered, in source order.
    ///
    /// Comments produce no tokens; this secondary stream exists for tools
    /// such as the formatter that must reproduce them. The scanning rules
    /// are identical to tokenization's, so a source with lexical errors may
    /// yield a partial comment list.
    pub fn tokenize_with_comments(
        &self,
        sink: &mut DiagnosticSink,
    ) -> (Vec<Token>, Vec<CommentTrivia>) {
        let contents = self.source.contents();
        let bytes = contents.as_bytes();
        let mut tokens = Vec::new();
        let mut comments = Vec::new();
        let mut import_context = ImportContext::None;
        // `:` introduces an annotation-only context. Type spellings are still
        // identifiers everywhere else, so existing v1 programs retain their
        // namespace unchanged.
        let mut awaits_type_name = false;
        let mut pos = 0;

        while pos < bytes.len() {
            let byte = bytes[pos];

            // Whitespace separates tokens and carries no meaning.
            if byte.is_ascii_whitespace() {
                pos += 1;
                continue;
            }

            // Line comments: `//` runs to the end of the line and is
            // ignored, so a comment may appear wherever whitespace may.
            if byte == b'/' && pos + 1 < bytes.len() && bytes[pos + 1] == b'/' {
                let start = pos;
                while pos < bytes.len() && bytes[pos] != b'\n' {
                    pos += 1;
                }
                comments.push(CommentTrivia {
                    kind: CommentKind::Line,
                    span: Span::new(start, pos),
                });
                continue;
            }

            // Block comments: `/*` ... `*/` may span multiple lines and
            // nest, so code containing block comments can itself be
            // commented out. An unterminated comment is an error.
            if byte == b'/' && pos + 1 < bytes.len() && bytes[pos + 1] == b'*' {
                let start = pos;
                let mut depth = 0usize;
                while pos < bytes.len() {
                    if bytes[pos] == b'/' && pos + 1 < bytes.len() && bytes[pos + 1] == b'*' {
                        depth += 1;
                        pos += 2;
                    } else if bytes[pos] == b'*' && pos + 1 < bytes.len() && bytes[pos + 1] == b'/'
                    {
                        depth -= 1;
                        pos += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        pos += 1;
                    }
                }
                if depth != 0 {
                    sink.emit(
                        Diagnostic::error("unterminated block comment")
                            .at(Span::new(start, contents.len())),
                    );
                } else {
                    comments.push(CommentTrivia {
                        kind: CommentKind::Block,
                        span: Span::new(start, pos),
                    });
                }
                continue;
            }

            // Identifiers and keywords: `[A-Za-z_][A-Za-z0-9_]*`.
            if byte == b'_' || byte.is_ascii_alphabetic() {
                let start = pos;
                pos += 1;
                while pos < bytes.len()
                    && (bytes[pos] == b'_' || bytes[pos].is_ascii_alphanumeric())
                {
                    pos += 1;
                }
                let text = &contents[start..pos];
                let mut kind = match text {
                    "let" => TokenKind::Keyword(Keyword::Let),
                    "true" => TokenKind::Keyword(Keyword::True),
                    "false" => TokenKind::Keyword(Keyword::False),
                    "if" => TokenKind::Keyword(Keyword::If),
                    "else" => TokenKind::Keyword(Keyword::Else),
                    "while" => TokenKind::Keyword(Keyword::While),
                    "for" => TokenKind::Keyword(Keyword::For),
                    "in" => TokenKind::Keyword(Keyword::In),
                    "fn" => TokenKind::Keyword(Keyword::Function),
                    "return" => TokenKind::Keyword(Keyword::Return),
                    "break" => TokenKind::Keyword(Keyword::Break),
                    "continue" => TokenKind::Keyword(Keyword::Continue),
                    "use" => TokenKind::Keyword(Keyword::Use),
                    _ => TokenKind::Ident,
                };
                if import_context == ImportContext::AwaitAs && text == "as" {
                    kind = TokenKind::ImportAs;
                    import_context = ImportContext::AwaitAlias;
                }
                if awaits_type_name {
                    if let Some(type_name) = TypeName::parse(text) {
                        kind = TokenKind::TypeName(type_name);
                    }
                    awaits_type_name = false;
                }
                tokens.push(Token {
                    kind,
                    span: Span::new(start, pos),
                });
                match kind {
                    TokenKind::Keyword(Keyword::Use) => import_context = ImportContext::AwaitPath,
                    TokenKind::ImportAs => {}
                    _ if import_context != ImportContext::None => {
                        import_context = ImportContext::None;
                    }
                    _ => {}
                }
                continue;
            }

            // Integer literals: `[0-9]+`.
            if byte.is_ascii_digit() {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Integer,
                    span: Span::new(start, pos),
                });
                if import_context != ImportContext::None {
                    import_context = ImportContext::None;
                }
                continue;
            }

            // String literals: `"..."` with `\n`, `\t`, `\\`, and `\"`
            // escape sequences. A raw newline may not appear inside a
            // string; an unterminated literal is reported and the token
            // still covers what was scanned so parsing can continue.
            if byte == b'"' {
                let start = pos;
                pos += 1;
                let mut terminated = false;
                while pos < bytes.len() {
                    match bytes[pos] {
                        b'"' => {
                            pos += 1;
                            terminated = true;
                            break;
                        }
                        b'\\' => match bytes.get(pos + 1) {
                            Some(b'n' | b't' | b'\\' | b'"') => pos += 2,
                            Some(_) => {
                                sink.emit(
                                    Diagnostic::error(format!(
                                        "unknown escape sequence `\\{}`",
                                        contents[pos + 1..]
                                            .chars()
                                            .next()
                                            .expect("a byte offset after `pos` is a char boundary")
                                    ))
                                    .at(Span::new(pos, pos + 2)),
                                );
                                pos += 2;
                            }
                            None => {
                                sink.emit(
                                    Diagnostic::error("unterminated escape sequence")
                                        .at(Span::new(pos, pos + 1)),
                                );
                                pos += 1;
                            }
                        },
                        b'\n' => break,
                        _ => pos += 1,
                    }
                }
                if !terminated {
                    sink.emit(
                        Diagnostic::error("unterminated string literal").at(Span::new(start, pos)),
                    );
                }
                tokens.push(Token {
                    kind: TokenKind::StringLiteral,
                    span: Span::new(start, pos),
                });
                if import_context == ImportContext::AwaitPath {
                    import_context = ImportContext::AwaitAs;
                } else if import_context != ImportContext::None {
                    import_context = ImportContext::None;
                }
                continue;
            }

            // Any ASCII punctuation character. Two-character operators
            // (`<=`, `>=`, `==`, `!=`) are matched with maximal munch, so
            // `<=` is one token while `<` followed by `=` (as in `a < b = c`)
            // remains two.
            if byte.is_ascii_punctuation() {
                let start = pos;
                pos += 1;
                let kind = match (byte, bytes.get(pos)) {
                    (b'<', Some(b'=')) => {
                        pos += 1;
                        TokenKind::LessEqual
                    }
                    (b'>', Some(b'=')) => {
                        pos += 1;
                        TokenKind::GreaterEqual
                    }
                    (b'=', Some(b'=')) => {
                        pos += 1;
                        TokenKind::EqualEqual
                    }
                    (b'!', Some(b'=')) => {
                        pos += 1;
                        TokenKind::NotEqual
                    }
                    (b'.', Some(b'.')) => {
                        pos += 1;
                        TokenKind::DotDot
                    }
                    _ => TokenKind::Punctuation(byte as char),
                };
                tokens.push(Token {
                    kind,
                    span: Span::new(start, pos),
                });
                if byte == b':' {
                    awaits_type_name = true;
                } else if awaits_type_name {
                    awaits_type_name = false;
                }
                if import_context != ImportContext::None {
                    import_context = ImportContext::None;
                }
                continue;
            }

            // Unrecognized character: report it and recover by skipping the
            // whole UTF-8 code point so `pos` stays on a char boundary.
            let ch = contents[pos..]
                .chars()
                .next()
                .expect("`pos` always points at a char boundary");
            let width = ch.len_utf8();
            sink.emit(
                Diagnostic::error(format!("unexpected character `{ch}`"))
                    .at(Span::new(pos, pos + width)),
            );
            pos += width;
        }

        tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(pos, pos),
        });

        (tokens, comments)
    }
}

/// Decodes the contents of a string literal into its runtime value.
///
/// `raw` is the lexeme as it appears in the source, including the
/// surrounding quotes. Escape sequences (`\n`, `\t`, `\\`, `\"`) are
/// decoded; an unknown escape sequence is passed through unchanged, since
/// the lexer has already reported it as an error.
pub fn unescape_string(raw: &str) -> String {
    let inner = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(raw);
    let mut decoded = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => decoded.push('\n'),
            Some('t') => decoded.push('\t'),
            Some('\\') => decoded.push('\\'),
            Some('"') => decoded.push('"'),
            Some(escaped) => {
                decoded.push('\\');
                decoded.push(escaped);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;

    /// Extracts the source text covered by a token's span.
    fn lexeme<'src>(source: &'src SourceFile, token: &Token) -> &'src str {
        source
            .slice(token.span)
            .expect("token spans are always valid")
    }

    #[test]
    fn empty_source_yields_only_eof() {
        let source = SourceFile::new("empty.ucl", "");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Eof);
        assert_eq!(tokens[0].span, Span::new(0, 0));
    }

    #[test]
    fn tokenizes_identifiers_integers_and_punctuation() {
        let source = SourceFile::new("main.ucl", "let answer = 42;");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());

        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Ident,
                TokenKind::Punctuation('='),
                TokenKind::Integer,
                TokenKind::Punctuation(';'),
                TokenKind::Eof,
            ]
        );
        assert_eq!(lexeme(&source, &tokens[0]), "let");
        assert_eq!(lexeme(&source, &tokens[1]), "answer");
        assert_eq!(lexeme(&source, &tokens[3]), "42");
    }

    #[test]
    fn tokenizes_for_headers_and_range_separators() {
        let source = SourceFile::new("main.ucl", "for i in 0..5 { i; };");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());

        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Keyword(Keyword::For),
                TokenKind::Ident,
                TokenKind::Keyword(Keyword::In),
                TokenKind::Integer,
                TokenKind::DotDot,
                TokenKind::Integer,
                TokenKind::Punctuation('{'),
                TokenKind::Ident,
                TokenKind::Punctuation(';'),
                TokenKind::Punctuation('}'),
                TokenKind::Punctuation(';'),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn a_single_dot_is_not_a_range_separator() {
        // Member access keeps its single `.`; only two adjacent dots form
        // the range separator.
        let source = SourceFile::new("main.ucl", "math.one;");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());

        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Ident,
                TokenKind::Punctuation('.'),
                TokenKind::Ident,
                TokenKind::Punctuation(';'),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_let_as_a_keyword() {
        let source = SourceFile::new("main.ucl", "let letx _let let");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());

        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Ident,
                TokenKind::Ident,
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_boolean_literals_as_keywords() {
        let source = SourceFile::new("main.ucl", "true false truex _true True");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());

        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Keyword(Keyword::True),
                TokenKind::Keyword(Keyword::False),
                TokenKind::Ident,
                TokenKind::Ident,
                TokenKind::Ident,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn skips_whitespace_and_records_spans() {
        let source = SourceFile::new("main.ucl", "  x \n 12 ");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());

        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![TokenKind::Ident, TokenKind::Integer, TokenKind::Eof]
        );
        assert_eq!(lexeme(&source, &tokens[0]), "x");
        assert_eq!(tokens[0].span, Span::new(2, 3));
        assert_eq!(tokens[1].span, Span::new(6, 8));
    }

    #[test]
    fn reports_unknown_characters_and_recovers() {
        let source = SourceFile::new("main.ucl", "let π = 3;");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(sink.has_errors());

        let diagnostics: Vec<_> = sink.iter().collect();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].span, Some(Span::new(4, 6)));

        // Recovery: the tokens after the bad character are still produced.
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Punctuation('='),
                TokenKind::Integer,
                TokenKind::Punctuation(';'),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_two_character_operators_with_maximal_munch() {
        let source = SourceFile::new("main.ucl", "a <= b >= c == d != e < f = g ! h");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());

        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Ident,
                TokenKind::LessEqual,
                TokenKind::Ident,
                TokenKind::GreaterEqual,
                TokenKind::Ident,
                TokenKind::EqualEqual,
                TokenKind::Ident,
                TokenKind::NotEqual,
                TokenKind::Ident,
                TokenKind::Punctuation('<'),
                TokenKind::Ident,
                TokenKind::Punctuation('='),
                TokenKind::Ident,
                TokenKind::Punctuation('!'),
                TokenKind::Ident,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn skips_line_comments() {
        let source = SourceFile::new("main.ucl", "let x = 1; // trailing comment");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Ident,
                TokenKind::Punctuation('='),
                TokenKind::Integer,
                TokenKind::Punctuation(';'),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn comment_only_source_yields_only_eof() {
        let source = SourceFile::new("main.ucl", "// nothing but a comment");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Eof);
    }

    #[test]
    fn skips_block_comments_including_across_lines() {
        let source = SourceFile::new("main.ucl", "1 /* hidden\nstill hidden */ + 2");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Integer,
                TokenKind::Punctuation('+'),
                TokenKind::Integer,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn block_comments_nest() {
        let source = SourceFile::new("main.ucl", "/* outer /* inner */ still outer */ 7;");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Integer));
    }

    #[test]
    fn an_unterminated_block_comment_is_an_error() {
        let source = SourceFile::new("main.ucl", "1; /* never closed");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("unterminated block comment"))
        );
        // The tokens before the comment still scan.
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Integer));
    }

    #[test]
    fn a_comment_opener_inside_a_string_literal_stays_literal() {
        let source = SourceFile::new("main.ucl", "\"/* not a comment */\";");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        assert!(
            tokens
                .iter()
                .any(|token| token.kind == TokenKind::StringLiteral)
        );
    }

    #[test]
    fn comment_ends_at_the_end_of_the_line() {
        let source = SourceFile::new("main.ucl", "1 // comment\n2");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![TokenKind::Integer, TokenKind::Integer, TokenKind::Eof]
        );
        // The token after the comment points at the `2` on the next line.
        assert_eq!(lexeme(&source, &tokens[1]), "2");
    }

    #[test]
    fn comment_content_is_ignored_even_when_it_looks_like_code() {
        let source = SourceFile::new("main.ucl", "// let x = 1; ( ) { } 42 // nested\nx");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(kinds, vec![TokenKind::Ident, TokenKind::Eof]);
        assert_eq!(lexeme(&source, &tokens[0]), "x");
    }

    #[test]
    fn single_slash_is_still_the_division_operator() {
        let source = SourceFile::new("main.ucl", "6 / 2");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Integer,
                TokenKind::Punctuation('/'),
                TokenKind::Integer,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn skips_non_ascii_content_inside_comments() {
        let source = SourceFile::new("main.ucl", "// café comment\n42");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(kinds, vec![TokenKind::Integer, TokenKind::Eof]);
        assert_eq!(lexeme(&source, &tokens[0]), "42");
    }

    #[test]
    fn recognizes_control_flow_and_function_keywords() {
        let source = SourceFile::new("main.ucl", "if elsex while fn iff _if fnx");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());

        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Keyword(Keyword::If),
                TokenKind::Ident,
                TokenKind::Keyword(Keyword::While),
                TokenKind::Keyword(Keyword::Function),
                TokenKind::Ident,
                TokenKind::Ident,
                TokenKind::Ident,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_string_literals_with_spans_including_quotes() {
        let source = SourceFile::new("main.ucl", "let x = \"hi there\";");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        assert_eq!(tokens[3].kind, TokenKind::StringLiteral);
        assert_eq!(lexeme(&source, &tokens[3]), "\"hi there\"");
    }

    #[test]
    fn accepts_the_supported_escape_sequences() {
        let source = SourceFile::new("main.ucl", "\"a\\n\\t\\\\\\\"b\"");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(!sink.has_errors());
        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
        assert_eq!(unescape_string(lexeme(&source, &tokens[0])), "a\n\t\\\"b");
    }

    #[test]
    fn reports_unknown_escape_sequences() {
        let source = SourceFile::new("main.ucl", "\"a\\qb\"");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(sink.has_errors());
        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
        let diagnostics: Vec<_> = sink.iter().collect();
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .message
                .contains("unknown escape sequence `\\q`")
        );
    }

    #[test]
    fn reports_unterminated_string_literals() {
        let source = SourceFile::new("main.ucl", "\"never closed");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("unterminated string literal"))
        );
        // The token still covers the scanned text so parsing can continue.
        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
        assert_eq!(lexeme(&source, &tokens[0]), "\"never closed");
    }

    #[test]
    fn a_string_may_not_span_a_raw_newline() {
        let source = SourceFile::new("main.ucl", "\"first\nlet x = 1;\"");
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);

        // Both lines produce an unterminated-string diagnostic.
        assert!(sink.has_errors());
        assert_eq!(
            sink.iter()
                .filter(|diagnostic| diagnostic.message.contains("unterminated string literal"))
                .count(),
            2
        );
        // Scanning recovers and tokenizes the rest of each line.
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::StringLiteral,
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Ident,
                TokenKind::Punctuation('='),
                TokenKind::Integer,
                TokenKind::Punctuation(';'),
                TokenKind::StringLiteral,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unescape_string_passes_text_through_without_escapes() {
        assert_eq!(unescape_string("\"plain\""), "plain");
        assert_eq!(unescape_string("\"\""), "");
        // Unknown escapes are left as-is; the lexer reports them separately.
        assert_eq!(unescape_string("\"a\\xb\""), "a\\xb");
    }

    #[test]
    fn recognizes_as_only_after_an_import_path() {
        let source = SourceFile::new("main.ucl", "let as = 1; use \"math.ucl\" as math; as;");
        let tokens = Lexer::new(&source).tokenize(&mut DiagnosticSink::new());
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();

        assert_eq!(
            kinds,
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Ident,
                TokenKind::Punctuation('='),
                TokenKind::Integer,
                TokenKind::Punctuation(';'),
                TokenKind::Keyword(Keyword::Use),
                TokenKind::StringLiteral,
                TokenKind::ImportAs,
                TokenKind::Ident,
                TokenKind::Punctuation(';'),
                TokenKind::Ident,
                TokenKind::Punctuation(';'),
                TokenKind::Eof,
            ]
        );
    }
}
