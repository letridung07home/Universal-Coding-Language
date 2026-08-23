//! Interactive read-eval-print loop for the `ucl` binary.
//!
//! The REPL keeps one [`ucl::Environment`] alive for the whole session so
//! bindings persist across inputs, and one [`ucl::Evaluator`] to run each
//! entry. Input that ends in the middle of a construct (detected via
//! [`ucl::Parser::is_incomplete`]) prompts a continuation line instead of
//! reporting an error.

use std::io::{self, BufRead, Write};

use ucl::{DiagnosticSink, Environment, Evaluator, Lexer, Parser, SourceFile};

use crate::render::{format_value, render_diagnostics};

/// The primary prompt shown before any input.
const PROMPT: &str = ">>> ";
/// The continuation prompt shown while an entry is syntactically incomplete.
const CONTINUATION_PROMPT: &str = "... ";

/// Runs the REPL until end of input or a quit command.
///
/// `search_paths` holds module search directories from `-p/--path` flags;
/// the process environment's `UCL_PATH` entries are appended here so both
/// entry points share one configuration order. The paths survive `:reset`.
pub fn run(search_paths: &[String]) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    println!(
        "UCL {} interactive mode — type :help for help.",
        env!("CARGO_PKG_VERSION")
    );

    let evaluator = Evaluator::new();
    let mut environment = fresh_environment(search_paths);

    loop {
        write!(stdout, "{PROMPT}")?;
        stdout.flush()?;

        let first_line = match read_line(&stdin)? {
            Some(line) => line,
            None => break,
        };
        if is_meta_command(&first_line) {
            if !handle_meta_command(first_line.trim(), &mut environment, search_paths) {
                break;
            }
            continue;
        }

        let mut buffer = first_line;
        // Keep reading continuation lines while the accumulated input still
        // ends in the middle of a construct. Each attempt uses its own sink
        // so diagnostics from discarded attempts are never printed.
        loop {
            let (incomplete, errors) = {
                let source = SourceFile::new("<repl>", buffer.clone());
                let mut sink = DiagnosticSink::new();
                let tokens = Lexer::new(&source).tokenize(&mut sink);
                let mut parser = Parser::new(tokens);
                parser.parse(&mut sink);
                (parser.is_incomplete(), sink.has_errors())
            };

            if !errors || !incomplete {
                break;
            }
            write!(stdout, "{CONTINUATION_PROMPT}")?;
            stdout.flush()?;
            match read_line(&stdin)? {
                Some(line) => {
                    buffer.push('\n');
                    buffer.push_str(&line);
                }
                None => return Ok(()),
            }
        }

        evaluate_entry(&evaluator, &mut environment, &buffer);
    }

    Ok(())
}

/// Evaluates one complete REPL entry against the session environment.
fn evaluate_entry(evaluator: &Evaluator, environment: &mut Environment, source_text: &str) {
    let source = SourceFile::new("<repl>", source_text);
    let mut sink = DiagnosticSink::new();

    let tokens = Lexer::new(&source).tokenize(&mut sink);
    let ast = if sink.has_errors() {
        render_diagnostics(&sink, &source);
        return;
    } else {
        Parser::new(tokens).parse(&mut sink)
    };

    let Some(ast) = ast else {
        render_diagnostics(&sink, &source);
        return;
    };
    if sink.has_errors() {
        render_diagnostics(&sink, &source);
        return;
    }

    match evaluator.evaluate_in(environment, &ast, &source, &mut sink) {
        Some(value) => {
            // Echo the result unless it is unit: declarations evaluate to
            // unit and stay silent, bare expressions print their value.
            if let Some(text) = format_value(&value) {
                println!("{text}");
            }
        }
        None => render_diagnostics(&sink, &source),
    }
}

/// Returns true when the line is a REPL meta command such as `:quit`.
fn is_meta_command(line: &str) -> bool {
    line.trim_start().starts_with(':')
}

/// Builds a fresh session environment with the given module search paths
/// plus any `UCL_PATH` directories from the process environment.
fn fresh_environment(search_paths: &[String]) -> Environment {
    let mut environment = Environment::new();
    for dir in search_paths {
        environment.add_search_path(dir);
    }
    if let Some(value) = std::env::var_os("UCL_PATH") {
        for dir in std::env::split_paths(&value) {
            environment.add_search_path(dir.display().to_string());
        }
    }
    environment
}

/// Handles a meta command. Returns false when the session should end.
fn handle_meta_command(
    command: &str,
    environment: &mut Environment,
    search_paths: &[String],
) -> bool {
    match command.trim() {
        ":help" => {
            println!(":help   show this help");
            println!(":reset  forget all bindings from this session");
            println!(":quit   exit the interpreter (also Ctrl-D)");
        }
        ":reset" => {
            *environment = fresh_environment(search_paths);
            println!("session reset");
        }
        ":quit" | ":exit" => return false,
        other => println!("unknown command `{other}`; type :help for help"),
    }
    true
}

/// Reads one line of input, returning `None` on end of input.
fn read_line(stdin: &io::Stdin) -> io::Result<Option<String>> {
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line)?;
    if read == 0 {
        println!();
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\n', '\r']).to_owned()))
}
