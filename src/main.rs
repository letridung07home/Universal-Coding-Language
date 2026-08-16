//! Command-line entry point for the `ucl` binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    // TODO(#1): parse arguments, load source files, and run the pipeline.
    eprintln!(
        "ucl {}: the compiler pipeline is not implemented yet.",
        env!("CARGO_PKG_VERSION")
    );
    ExitCode::SUCCESS
}
