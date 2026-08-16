//! Command-line entry point for the `ucl` binary.

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
    let tokens = Lexer::new(&source).tokenize(&mut sink);
    let ast = Parser::new(tokens).parse(&mut sink);
    let value = ast.map_or(Value::Unit, |ast| {
        Evaluator::new().evaluate(&ast, &source, &mut sink)
    });

    render_diagnostics(&sink, &source);

    if sink.has_errors() {
        return ExitCode::FAILURE;
    }

    print_value(&value);
    ExitCode::SUCCESS
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
        let label = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };

        eprintln!("{label}: {}", diagnostic.message);
        if let Some(span) = diagnostic.span {
            let (line, column) = line_column(source, span.start);
            eprintln!("  --> {}:{line}:{column}", source.name());
        }
    }
}

/// Converts a byte offset into a 1-based `(line, column)` pair.
fn line_column(source: &SourceFile, offset: usize) -> (usize, usize) {
    let contents = source.contents();
    let offset = offset.min(contents.len());
    let line_start = contents[..offset]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let line = contents[..line_start]
        .bytes()
        .filter(|&byte| byte == b'\n')
        .count()
        + 1;
    let column = contents[line_start..offset].chars().count() + 1;
    (line, column)
}

/// Prints the program's result, omitting the unit value.
fn print_value(value: &Value) {
    match value {
        Value::Unit => {}
        Value::Integer(integer) => println!("{integer}"),
        Value::Boolean(boolean) => println!("{boolean}"),
    }
}
