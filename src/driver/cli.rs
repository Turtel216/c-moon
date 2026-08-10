//! Command line interface defintion

use clap::Parser;

/// Command line interface flags
#[derive(Parser)]
pub struct Cli {
    /// The C source file
    pub source_file: String,

    /// The output file.
    #[arg(short, default_value_t = String::from("output"))]
    pub output_file: String,

    /// Enable optimizations
    #[arg(long, default_value_t = false)]
    pub opt: bool, // TODO: find better name, cant use O because of conflict with output_file

    /// Pretty print AST
    #[arg(long, default_value_t = false)]
    pub printast: bool,

    /// Pretty print IR
    #[arg(long, default_value_t = false)]
    pub printir: bool,

    /// Output Assembly
    #[arg(long, default_value_t = false)]
    pub asm: bool,
}

/// Get command line arguments
pub fn get_arguments() -> Cli {
    let cli = Cli::parse();
    cli
}
