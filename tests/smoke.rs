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

#[test]
fn invalid_spans_do_not_panic() {
    let source = SourceFile::new("test.ucl", "hello");

    // Inverted and out-of-range spans are reported, not panics.
    assert_eq!(source.slice(Span::new(2, 1)), None);
    assert_eq!(source.slice(Span::new(0, 100)), None);
    assert_eq!(Span::new(5, 3).len(), 0);
}
