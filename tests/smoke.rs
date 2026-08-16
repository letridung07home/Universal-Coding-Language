//! Basic integration smoke tests for the public API.

use ucl::{SourceFile, Span};

#[test]
fn source_file_round_trips_name_and_contents() {
    let source = SourceFile::new("test.ucl", "let answer = 42;");
    assert_eq!(source.name(), "test.ucl");
    assert_eq!(source.contents(), "let answer = 42;");
}

#[test]
fn spans_report_their_length() {
    let span = Span::new(0, 3);
    assert_eq!(span.len(), 3);
    assert!(!span.is_empty());
}
