//! Diagnostic and value rendering shared by the file CLI and the REPL.

use ucl::{Diagnostic, DiagnosticSink, Severity, SourceFile, Value};

/// Prints every recorded diagnostic to stderr.
pub fn render_diagnostics(sink: &DiagnosticSink, source: &SourceFile) {
    for diagnostic in sink.iter() {
        eprint!("{}", format_diagnostic(diagnostic, source));
    }
}

/// Formats one diagnostic, including the source excerpt for an anchored span.
pub fn format_diagnostic(diagnostic: &Diagnostic, source: &SourceFile) -> String {
    let label = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    };

    let mut output = format!("{label}: {}\n", diagnostic.message);
    let Some(span) = diagnostic.span else {
        return output;
    };

    let contents = source.contents();
    let start = char_boundary(contents, span.start);
    let end = char_boundary(contents, span.end.max(start));
    let (start_line, column) = line_column(contents, start);
    output.push_str(&format!("  --> {}:{start_line}:{column}\n", source.name()));
    output.push_str("   |\n");

    let last_line = if start == end {
        start_line
    } else {
        line_column(contents, end.saturating_sub(1)).0
    };
    let line_number_width = last_line.to_string().len();
    let mut line_start = line_start(contents, start);

    for line in start_line..=last_line {
        let line_end = contents[line_start..]
            .find('\n')
            .map_or(contents.len(), |newline| line_start + newline);
        let display_end = contents[line_start..line_end]
            .strip_suffix('\r')
            .map_or(line_end, |line| line_start + line.len());
        let marker_start = if line == start_line {
            start
        } else {
            line_start
        };
        let marker_end = if line == last_line { end } else { display_end };
        let marker_start = marker_start.clamp(line_start, display_end);
        let marker_end = marker_end.clamp(marker_start, display_end);

        output.push_str(&format!(
            "  {line:>line_number_width$} | {}\n",
            &contents[line_start..display_end]
        ));
        output.push_str(&format!(
            "  {:>line_number_width$} | {}{}\n",
            "",
            marker_prefix(&contents[line_start..marker_start]),
            marker_text(&contents[marker_start..marker_end])
        ));

        if line < last_line {
            line_start = line_end + 1;
        }
    }

    output
}

/// Converts a byte offset into a 1-based `(line, column)` pair.
fn line_column(contents: &str, offset: usize) -> (usize, usize) {
    let offset = char_boundary(contents, offset);
    let line_start = line_start(contents, offset);
    let line = contents[..line_start]
        .bytes()
        .filter(|&byte| byte == b'\n')
        .count()
        + 1;
    let column = contents[line_start..offset].chars().count() + 1;
    (line, column)
}

/// Returns the byte offset at which the line containing `offset` begins.
fn line_start(contents: &str, offset: usize) -> usize {
    contents[..char_boundary(contents, offset)]
        .rfind('\n')
        .map_or(0, |newline| newline + 1)
}

/// Clamps an offset to a valid UTF-8 character boundary in `contents`.
fn char_boundary(contents: &str, offset: usize) -> usize {
    let mut offset = offset.min(contents.len());
    while offset > 0 && !contents.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Preserves tabs before a diagnostic marker so it stays aligned with source.
fn marker_prefix(source: &str) -> String {
    source
        .chars()
        .map(|character| if character == '\t' { '\t' } else { ' ' })
        .collect()
}

/// Creates the carets for the part of a span on one source line.
fn marker_text(source: &str) -> String {
    let width = source.chars().count().max(1);
    "^".repeat(width)
}

/// Renders a value the way the CLI echoes results, omitting the unit value.
pub fn format_value(value: &Value) -> Option<String> {
    match value {
        Value::Unit => None,
        Value::Integer(integer) => Some(format!("{integer}")),
        Value::Boolean(boolean) => Some(format!("{boolean}")),
        Value::Str(string) => Some(string.clone()),
        Value::Function(_) => Some("<function>".to_owned()),
    }
}
