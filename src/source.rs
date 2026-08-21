//! Source text handling: files, byte positions, and spans.

/// A byte offset into a [`SourceFile`].
///
/// All positions within source files are represented as byte offsets,
/// which allows efficient indexing and span calculations.
pub type BytePos = usize;

/// A single source file, held in memory as UTF-8 text.
///
/// A [`SourceFile`] owns its name (usually a path) and its contents. All
/// positions and [`Span`]s are expressed as byte offsets into `contents`.
pub struct SourceFile {
    name: String,
    contents: String,
}

impl SourceFile {
    /// Creates a source file from a name and its UTF-8 contents.
    pub fn new(name: impl Into<String>, contents: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            contents: contents.into(),
        }
    }

    /// The file's name, usually a path.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The file's contents.
    pub fn contents(&self) -> &str {
        &self.contents
    }

    /// Returns the source text covered by the given [`Span`].
    ///
    /// This is a convenience method for extracting the substring corresponding
    /// to a span's byte range. It returns `None` rather than panicking when
    /// the span is invalid: when its start is after its end, when either
    /// offset lies outside the file's contents, or when either offset does not
    /// fall on a UTF-8 character boundary.
    pub fn slice(&self, span: Span) -> Option<&str> {
        let valid = span.start <= span.end
            && span.end <= self.contents.len()
            && self.contents.is_char_boundary(span.start)
            && self.contents.is_char_boundary(span.end);
        valid.then(|| &self.contents[span.start..span.end])
    }
}

/// A half-open range of bytes `start..end` within a [`SourceFile`].
///
/// `start` is inclusive and `end` is exclusive. Spans do not carry a
/// reference to their file; callers track that separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    /// Byte offset of the first byte (inclusive).
    pub start: BytePos,
    /// Byte offset one past the last byte (exclusive).
    pub end: BytePos,
}

impl Span {
    /// Creates a new span from inclusive `start` and exclusive `end` offsets.
    pub const fn new(start: BytePos, end: BytePos) -> Self {
        Self { start, end }
    }

    /// The number of bytes covered by this span.
    ///
    /// This is saturating: an invalid span whose start is after its end
    /// reports a length of `0` instead of underflowing.
    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether this span covers zero bytes.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_file_exposes_name_and_contents() {
        let source = SourceFile::new("main.ucl", "let x = 1");
        assert_eq!(source.name(), "main.ucl");
        assert_eq!(source.contents(), "let x = 1");
    }

    #[test]
    fn span_len_is_end_minus_start() {
        let span = Span::new(4, 9);
        assert_eq!(span.len(), 5);
        assert!(!span.is_empty());
    }

    #[test]
    fn span_len_saturates_for_inverted_spans() {
        let span = Span::new(5, 3);
        assert_eq!(span.len(), 0);
        assert!(!span.is_empty());
    }

    #[test]
    fn slice_returns_the_text_for_valid_spans() {
        let source = SourceFile::new("main.ucl", "héllo");
        assert_eq!(source.slice(Span::new(0, 3)), Some("hé"));
        assert_eq!(source.slice(Span::new(3, 6)), Some("llo"));
        assert_eq!(source.slice(Span::new(6, 6)), Some(""));
    }

    #[test]
    fn slice_returns_none_for_invalid_spans() {
        let source = SourceFile::new("main.ucl", "héllo");

        // Inverted span.
        assert_eq!(source.slice(Span::new(3, 1)), None);
        // End offset outside the contents.
        assert_eq!(source.slice(Span::new(0, 7)), None);
        // Offset that splits a multi-byte character.
        assert_eq!(source.slice(Span::new(1, 2)), None);
    }
}
