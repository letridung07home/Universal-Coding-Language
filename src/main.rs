//! Command-line entry point for the `ucl` binary.
//!
//! This module provides the CLI interface for evaluating UCL source files,
//! inline programs (`-e/--eval`), piped stdin, batch static checking, and
//! interactive sessions. It orchestrates the compiler pipeline
//! (lexer → parser → evaluator) and handles diagnostic formatting and output.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ucl::fmt::format_source;
use ucl::module::resolved_import_graph;
use ucl::{DiagnosticSink, Environment, Evaluator, Lexer, Parser, SourceFile};

mod render;
mod repl;

use render::{format_value, render_diagnostics};

/// Usage text printed on argument errors and `--help`.
const USAGE: &str = "usage: ucl [-p <dir>]... [-e <code> | <file>] [--list-imports | --type-check] [--strict-types]\n       ucl check [-p <dir>]... [--strict-types] <file>...\n       ucl fmt [--check] [<file> | -]";

/// The environment variable holding module search directories, separated by
/// the platform's path separator (`:` on Unix, `;` on Windows).
const SEARCH_PATH_ENV: &str = "UCL_PATH";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    if args.get(1).map(String::as_str) == Some("fmt") {
        return run_fmt(&args[2..]);
    }
    if args.get(1).map(String::as_str) == Some("check") {
        return run_check(&args[2..]);
    }

    let ProgramArgs {
        input,
        mut search_paths,
        list_imports,
        type_check,
        strict_types,
    } = match parse_args(&args) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    search_paths.extend(env_search_paths());

    let (name, contents) = match read_input(&input) {
        Ok(parts) => parts,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    if list_imports {
        run_list_imports(&name, contents, &search_paths)
    } else {
        run_program(&name, contents, &search_paths, type_check, strict_types)
    }
}

/// Where program input comes from.
enum Input {
    File(String),
    Stdin,
    Eval(String),
}

/// Parsed arguments for ordinary program evaluation or inspection.
struct ProgramArgs {
    input: Input,
    search_paths: Vec<String>,
    list_imports: bool,
    type_check: bool,
    strict_types: bool,
}

/// Parsed arguments for `ucl fmt`.
struct FmtArgs {
    check: bool,
    input: Option<Input>,
}

/// Parsed arguments for `ucl check`.
struct CheckArgs {
    files: Vec<String>,
    search_paths: Vec<String>,
    strict_types: bool,
}

/// Runs `ucl check` over one or more entry files without evaluating source.
///
/// The command first resolves each complete import graph, then lexes, parses,
/// and type-checks every unique source file in that graph. This makes the
/// command suitable for CI and editor workflows: imported code is validated
/// but never executed. Exit code 0 means every file checked successfully, 1
/// reports source/type errors, and 2 reports usage or input errors.
fn run_check(args: &[String]) -> ExitCode {
    if matches!(args, [flag] if flag == "-h" || flag == "--help") {
        print_check_help();
        return ExitCode::SUCCESS;
    }

    let parsed = match parse_check_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let mut search_paths = parsed.search_paths;
    search_paths.extend(env_search_paths());
    let path_bufs = search_paths.iter().map(PathBuf::from).collect::<Vec<_>>();
    let mut checked = HashSet::new();
    let mut failed = false;

    for file in parsed.files {
        let contents = match fs::read_to_string(&file) {
            Ok(contents) => contents,
            Err(error) => {
                eprintln!("error: cannot read `{file}`: {error}");
                return ExitCode::from(2);
            }
        };
        let root_source = SourceFile::new(&file, contents);
        let mut graph_sink = DiagnosticSink::new();
        let Some(graph) = resolved_import_graph(&root_source, &path_bufs, &mut graph_sink) else {
            render_diagnostics(&graph_sink, &root_source);
            failed = true;
            continue;
        };

        let mut paths = Vec::with_capacity(graph.edges.len() + 1);
        paths.push(graph.root);
        for edge in graph.edges {
            paths.push(edge.imported);
        }

        for path in paths {
            let canonical = path.canonicalize().unwrap_or(path);
            if !checked.insert(canonical.clone()) {
                continue;
            }
            let source_contents = match fs::read_to_string(&canonical) {
                Ok(contents) => contents,
                Err(error) => {
                    eprintln!("error: cannot read `{}`: {error}", canonical.display());
                    return ExitCode::from(2);
                }
            };
            if !check_source_file(&canonical, source_contents, parsed.strict_types) {
                failed = true;
            }
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Checks one source file without evaluating it.
fn check_source_file(path: &Path, contents: String, strict_types: bool) -> bool {
    let name = path.display().to_string();
    let source = SourceFile::new(&name, contents);
    let mut sink = DiagnosticSink::new();
    let tokens = Lexer::new(&source).tokenize(&mut sink);
    let mut success = false;
    if !sink.has_errors()
        && let Some(ast) = Parser::new(tokens).parse(&mut sink)
        && !sink.has_errors()
    {
        let evaluator = Evaluator::new();
        let mut context = ucl::TypeContext::new();
        success = evaluator.type_check(&ast, &source, &mut context, &mut sink, strict_types);
    }
    render_diagnostics(&sink, &source);
    success && !sink.has_errors()
}

/// Prints help for `ucl check`.
fn print_check_help() {
    println!("usage: ucl check [-p <dir>]... [--strict-types] <file>...");
    println!();
    println!(
        "Parse and statically check one or more UCL entry files without evaluation."
    );
    println!("Imported modules are resolved and checked too, but never executed.");
    println!();
    println!("Options:");
    println!("  -p, --path <dir>  add a module search directory (repeatable)");
    println!("      --strict-types require complete function annotations");
    println!("  -h, --help        show this help");
}

/// Parses `ucl check` arguments.
fn parse_check_args(args: &[String]) -> Result<CheckArgs, String> {
    let mut files = Vec::new();
    let mut search_paths = Vec::new();
    let mut strict_types = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "-p" | "--path" => {
                let flag = args[index].clone();
                index += 1;
                let Some(directory) = args.get(index) else {
                    return Err(format!("`{flag}` requires a directory argument"));
                };
                search_paths.push(directory.clone());
            }
            "--strict-types" => {
                if strict_types {
                    return Err("repeated `--strict-types` flag".to_owned());
                }
                strict_types = true;
            }
            "-h" | "--help" => {
                return Err("`--help` must be used on its own".to_owned());
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}`")),
            file => files.push(file.to_owned()),
        }
        index += 1;
    }

    if files.is_empty() {
        return Err("`ucl check` expects at least one source file".to_owned());
    }

    Ok(CheckArgs {
        files,
        search_paths,
        strict_types,
    })
}

/// Runs `ucl fmt`.
fn run_fmt(args: &[String]) -> ExitCode {
    let Some(parsed) = parse_fmt_args(args) else {
        return ExitCode::from(2);
    };
    let Some(input) = parsed.input else {
        eprintln!("error: `ucl fmt` expects a file or `-` for standard input");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let (name, contents) = match read_input(&input) {
        Ok(parts) => parts,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    match format_source(&name, &contents) {
        Ok(formatted) => {
            if formatted == contents {
                return ExitCode::SUCCESS;
            }
            if parsed.check {
                println!("{name}: not formatted");
                return ExitCode::from(1);
            }
            match input {
                Input::Stdin => print!("{formatted}"),
                Input::File(ref file) => {
                    if let Err(error) = fs::write(file, &formatted) {
                        eprintln!("error: cannot write `{file}`: {error}");
                        return ExitCode::from(2);
                    }
                }
                Input::Eval(_) => unreachable!("`ucl fmt` never accepts inline programs"),
            }
            ExitCode::SUCCESS
        }
        Err(sink) => {
            let source = SourceFile::new(&name, contents);
            render_diagnostics(&sink, &source);
            ExitCode::from(1)
        }
    }
}

/// Parses formatter arguments.
fn parse_fmt_args(args: &[String]) -> Option<FmtArgs> {
    let mut parsed = FmtArgs {
        check: false,
        input: None,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--check" => {
                if parsed.check {
                    eprintln!("error: repeated `--check` flag");
                    return None;
                }
                parsed.check = true;
            }
            "-" => {
                if parsed.input.is_some() {
                    eprintln!("error: expected a single source file");
                    return None;
                }
                parsed.input = Some(Input::Stdin);
            }
            arg if !arg.starts_with('-') => {
                if parsed.input.is_some() {
                    eprintln!("error: expected a single source file");
                    return None;
                }
                parsed.input = Some(Input::File(arg.to_owned()));
            }
            other => {
                eprintln!("error: unknown option `{other}`");
                return None;
            }
        }
        index += 1;
    }
    Some(parsed)
}

/// Reads source text for one input.
fn read_input(input: &Input) -> Result<(String, String), String> {
    match input {
        Input::File(file) => {
            let contents = fs::read_to_string(file)
                .map_err(|error| format!("cannot read `{file}`: {error}"))?;
            Ok((file.clone(), contents))
        }
        Input::Stdin => {
            let mut contents = String::new();
            std::io::stdin()
                .read_to_string(&mut contents)
                .map_err(|error| format!("cannot read standard input: {error}"))?;
            Ok(("<stdin>".to_owned(), contents))
        }
        Input::Eval(code) => Ok(("<eval>".to_owned(), code.clone())),
    }
}

/// Runs the compiler pipeline over one program.
fn run_program(
    name: &str,
    contents: String,
    search_paths: &[String],
    type_check_only: bool,
    strict_types: bool,
) -> ExitCode {
    let source = SourceFile::new(name, contents);
    let mut sink = DiagnosticSink::new();

    let mut environment = Environment::default();
    let mut type_context = ucl::TypeContext::new();
    for dir in search_paths {
        environment.add_search_path(dir);
    }

    let mut value = None;
    let tokens = Lexer::new(&source).tokenize(&mut sink);
    if !sink.has_errors()
        && let Some(ast) = Parser::new(tokens).parse(&mut sink)
        && !sink.has_errors()
    {
        let evaluator = Evaluator::new();
        if type_check_only {
            if evaluator.type_check(&ast, &source, &mut type_context, &mut sink, strict_types) {
                value = Some(ucl::Value::Unit);
            }
        } else if strict_types {
            value = evaluator.evaluate_typed_in(&mut environment, &ast, &source, &mut sink);
        } else {
            value = evaluator.evaluate_in(&mut environment, &ast, &source, &mut sink);
        }
    }

    render_diagnostics(&sink, &source);

    match value {
        Some(value) => {
            if let Some(text) = format_value(&value) {
                println!("{text}");
            }
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

/// Prints one program's resolved import graph.
fn run_list_imports(name: &str, contents: String, search_paths: &[String]) -> ExitCode {
    let source = SourceFile::new(name, contents);
    let mut sink = DiagnosticSink::new();
    let paths = search_paths.iter().map(PathBuf::from).collect::<Vec<_>>();
    let Some(graph) = resolved_import_graph(&source, &paths, &mut sink) else {
        render_diagnostics(&sink, &source);
        return ExitCode::FAILURE;
    };

    println!("{}", graph.root.display());
    for edge in graph.edges {
        println!("{} -> {}", edge.importer.display(), edge.imported.display());
    }
    ExitCode::SUCCESS
}

/// Parses ordinary CLI arguments.
fn parse_args(args: &[String]) -> Result<Option<ProgramArgs>, String> {
    let mut file = None;
    let mut eval = None;
    let mut search_paths = Vec::new();
    let mut list_imports = false;
    let mut type_check = false;
    let mut strict_types = false;

    let mut index = 1;
    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;
        match arg {
            "-h" | "--help" => {
                println!("{USAGE}");
                println!();
                println!("Evaluate a Universal Coding Language program.");
                println!("Run without arguments to start an interactive session.");
                println!();
                println!("Options:");
                println!("  -e, --eval <code> evaluate inline program text");
                println!("  -p, --path <dir>  add a module search directory (repeatable)");
                println!(
                    "      --list-imports print the resolved import graph without evaluating source"
                );
                println!("      --type-check   check static types without evaluating source");
                println!(
                    "      --strict-types require annotated function signatures and check before evaluation"
                );
                println!("  -h, --help        show this help");
                println!("  -V, --version     show the version");
                println!();
                println!("Batch checker:");
                println!("  ucl check [-p <dir>]... [--strict-types] <file>...");
                println!("      check entry files and their imports without evaluation");
                println!();
                println!("Formatter:");
                println!("  ucl fmt [--check] [<file> | -]");
                println!("      format a file in place, or pipe stdin to stdout;");
                println!("      `--check` exits 1 when the input is not formatted");
                println!();
                println!(
                    "A file name of `-` reads the program from standard input; module imports also consult {SEARCH_PATH_ENV} directories (see https://github.com/letridung07home/Universal-Coding-Language)."
                );
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("ucl {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-e" | "--eval" => {
                let Some(code) = args.get(index) else {
                    return Err(format!("`{arg}` requires program text"));
                };
                if eval.is_some() || file.as_deref() == Some("-") {
                    return Err("expected a single `--eval` program".to_owned());
                }
                eval = Some(code.clone());
                index += 1;
            }
            "-p" | "--path" => {
                let Some(directory) = args.get(index) else {
                    return Err(format!("`{arg}` requires a directory argument"));
                };
                search_paths.push(directory.clone());
                index += 1;
            }
            "--list-imports" => {
                if list_imports {
                    return Err("repeated `--list-imports` flag".to_owned());
                }
                list_imports = true;
            }
            "--type-check" => {
                if type_check {
                    return Err("repeated `--type-check` flag".to_owned());
                }
                type_check = true;
            }
            "--strict-types" => {
                if strict_types {
                    return Err("repeated `--strict-types` flag".to_owned());
                }
                strict_types = true;
            }
            "-" => {
                if file.is_some() {
                    return Err("expected a single source file".to_owned());
                }
                if eval.is_some() {
                    return Err("cannot combine `--eval` with standard input".to_owned());
                }
                file = Some("-".to_owned());
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}`")),
            path => {
                if file.is_some() {
                    return Err("expected a single source file".to_owned());
                }
                file = Some(path.to_owned());
            }
        }
    }

    match (file, eval) {
        (Some(_), Some(_)) => Err("cannot combine `--eval` with a source file".to_owned()),
        _ if list_imports && (type_check || strict_types) => {
            Err("cannot combine `--list-imports` with type-checking flags".to_owned())
        }
        (Some(file), None) => {
            let input = if file == "-" {
                Input::Stdin
            } else {
                Input::File(file)
            };
            Ok(Some(ProgramArgs {
                input,
                search_paths,
                list_imports,
                type_check,
                strict_types,
            }))
        }
        (None, Some(code)) => Ok(Some(ProgramArgs {
            input: Input::Eval(code),
            search_paths,
            list_imports,
            type_check,
            strict_types,
        })),
        (None, None) if list_imports => {
            Err("`--list-imports` requires a source file, `-`, or `--eval` program".to_owned())
        }
        (None, None) => repl::run(&search_paths).map_or_else(
            |error| Err(format!("interactive session failed: {error}")),
            |_| Ok(None),
        ),
    }
}

/// Reads `UCL_PATH`, splitting it on the platform's path separator.
fn env_search_paths() -> Vec<String> {
    env::var_os(SEARCH_PATH_ENV)
        .map(|value| {
            env::split_paths(&value)
                .filter(|path| !path.as_os_str().is_empty())
                .map(|path| path.display().to_string())
                .collect()
        })
        .unwrap_or_default()
}
