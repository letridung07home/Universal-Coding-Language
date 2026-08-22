//! Command-line entry point for the `ucl` binary.
//!
//! This module provides the CLI interface for evaluating UCL source files.
//! It orchestrates the compiler pipeline (lexer → parser → evaluator)
//! and handles diagnostic formatting and output.

use std::env;
use std::fs;
use std::process::ExitCode;

use ucl::{DiagnosticSink, Evaluator, Lexer, Parser, Severity, SourceFile, Value};

/// Usage text printed on argument errors and `--help`.
const USAGE: &str = "usage: ucl <file>";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    let file = match parse_args(&args) {
        Ok(Some(file)) => file,
        // `--help` and `--version` already printed their output.
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let contents = match fs::read_to_string(&file) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!("error: cannot read `{file}`: {error}");
            return ExitCode::from(2);
        }
    };

    let source = SourceFile::new(file, contents);
    let mut sink = DiagnosticSink::new();

    // Run the pipeline: source text → tokens → AST → value.
    //
    // Each stage runs only if every earlier stage succeeded, so a program
    // with lexical errors is never parsed and one with syntax errors is
    // never evaluated. This keeps diagnostics focused on the root cause
    // instead of cascading through downstream stages fed garbage input.
    let mut value = None;
    let tokens = Lexer::new(&source).tokenize(&mut sink);
    if !sink.has_errors()
        && let Some(ast) = Parser::new(tokens).parse(&mut sink)
        && !sink.has_errors()
    {
        value = Evaluator::new().evaluate(&ast, &source, &mut sink);
    }

    render_diagnostics(&sink, &source);

    match value {
        Some(value) => {
            print_value(&value);
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

/// Interprets command-line arguments.
///
/// Returns the path of the source file to evaluate, or `None` after handling a
/// `--help` or `--version` flag. Usage errors are returned as messages.
fn parse_args(args: &[String]) -> Result<Option<String>, String> {
    let mut file = None;

    for arg in &args[1..] {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                println!();
                println!("Evaluate a Universal Coding Language source file.");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("ucl {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown option `{flag}`"));
            }
            path => {
                if file.is_some() {
                    return Err("expected a single source file".to_owned());
                }
                file = Some(path.to_owned());
            }
        }
    }

    match file {
        Some(file) => Ok(Some(file)),
        None => Err("no input file".to_owned()),
    }
}

/// Prints every recorded diagnostic to stderr.
fn render_diagnostics(sink: &DiagnosticSink, source: &SourceFile) {
    for diagnostic in sink.iter() {
        eprint!("{}", format_diagnostic(diagnostic, source));
    }
}

/// Formats one diagnostic, including the source excerpt for an anchored span.
fn format_diagnostic(diagnostic: &ucl::Diagnostic, source: &SourceFile) -> String {
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

/// Prints the program's result, omitting the unit value.
fn print_value(value: &Value) {
    match value {
        Value::Unit => {}
        Value::Integer(integer) => println!("{integer}"),
        Value::Boolean(boolean) => println!("{boolean}"),
        Value::Str(string) => println!("{string}"),
        Value::Function(_) => println!("<function>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ucl::{Diagnostic, Severity, Span};

    #[test]
    fn formats_a_single_line_excerpt() {
        let source = SourceFile::new("main.ucl", "1 / 0;");
        let diagnostic =
            Diagnostic::new(Severity::Error, Some(Span::new(0, 5)), "division by zero");

        assert_eq!(
            format_diagnostic(&diagnostic, &source),
            "error: division by zero\n  --> main.ucl:1:1\n   |\n  1 | 1 / 0;\n    | ^^^^^\n"
        );
    }

    #[test]
    fn formats_all_lines_touched_by_a_multiline_span() {
        let source = SourceFile::new("main.ucl", "1 /\n  0;");
        let diagnostic =
            Diagnostic::new(Severity::Error, Some(Span::new(0, 7)), "division by zero");
        let rendered = format_diagnostic(&diagnostic, &source);

        assert!(rendered.contains("  --> main.ucl:1:1"));
        assert!(rendered.contains("  1 | 1 /"));
        assert!(rendered.contains("  2 |   0;"));
        assert!(rendered.contains("    | ^^^"));
        assert!(rendered.contains("    | ^^^"));
    }

    #[test]
    fn formats_unanchored_diagnostics_without_an_excerpt() {
        let source = SourceFile::new("main.ucl", "");
        let diagnostic = Diagnostic::new(Severity::Note, None, "additional context");

        assert_eq!(
            format_diagnostic(&diagnostic, &source),
            "note: additional context\n"
        );
    }

    #[test]
    fn counts_columns_in_characters_not_bytes() {
        // `é` is two bytes but one character, so the `0` on line 2 sits at
        // character column 8, not at its byte offset within the line.
        let source = SourceFile::new("main.ucl", "let café = 5;\ncafé / 0;");
        let zero = source.contents().rfind('0').expect("source contains a `0`");
        let diagnostic = Diagnostic::new(
            Severity::Error,
            Some(Span::new(zero, zero + 1)),
            "division by zero",
        );

        let rendered = format_diagnostic(&diagnostic, &source);

        assert!(rendered.contains("  --> main.ucl:2:8"), "got: {rendered}");
        assert!(rendered.contains("  2 | café / 0;"), "got: {rendered}");
    }
}
