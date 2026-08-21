//! Lexer: turns source text into a stream of tokens.

use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::source::{SourceFile, Span};

/// The kind of a lexical token.
///
/// This vocabulary is intentionally small for now and will grow as the
/// language specification is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// An identifier or keyword: `foo`, `let`, `if`, etc.
    ///
    /// Note: The lexer does not distinguish between keywords and identifiers;
    /// the parser recognizes keyword patterns from the token stream.
    Ident,
    /// An integer literal: `42`, `123`, etc.
    Integer,
    /// A punctuation character: `(`, `)`, `{`, `}`, `+`, `-`, etc.
    Punctuation(char),
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

/// Converts source text into a vector of [`Token`]s.
///
/// The lexer performs lexical analysis, breaking source text into tokens
/// that the parser can use to build an abstract syntax tree.
pub struct Lexer<'src> {
    /// The source file being tokenized.
    source: &'src SourceFile,
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
        let contents = self.source.contents();
        let bytes = contents.as_bytes();
        let mut tokens = Vec::new();
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
                while pos < bytes.len() && bytes[pos] != b'\n' {
                    pos += 1;
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
                tokens.push(Token {
                    kind: TokenKind::Ident,
                    span: Span::new(start, pos),
                });
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
                continue;
            }

            // Any ASCII punctuation character.
            if byte.is_ascii_punctuation() {
                let start = pos;
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::Punctuation(byte as char),
                    span: Span::new(start, pos),
                });
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

        tokens
    }
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
                TokenKind::Ident,
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
                TokenKind::Ident,
                TokenKind::Punctuation('='),
                TokenKind::Integer,
                TokenKind::Punctuation(';'),
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
                TokenKind::Ident,
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
}
