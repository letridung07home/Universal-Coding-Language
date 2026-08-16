//! Source text handling: files, byte positions, and spans.

/// A byte offset into a [`SourceFile`].
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
    pub const fn len(self) -> usize {
        self.end - self.start
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
}
