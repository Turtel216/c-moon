//! Compiler Driver.

use std::{fs, process::Command};

use cli::get_arguments;
use diagnostics::Diagnostics;

use crate::{
    backend,
    frontend::{lexer::Lexer, parser::Parser, renamer::resolve_names, semantic::SemanticAnalyzer},
    middle::desuger::LoweringContext,
    printer::{ast_printer::AstPrinter, ir_printer::IrPrinter},
};

mod cli;
pub mod diagnostics;

/// Run the complete Compiler pipeline.
/// Also Handles command line arguments.
pub fn run() -> () {
    // Get command line arguments
    let cli = get_arguments();
    let mut diagnostics = Diagnostics::new();

    // Read source file
    // TODO: Dont crash the program when file not. Print proper message
    let source_program = fs::read_to_string(cli.source_file).expect("File not found");

    // Tokenize program
    let lexer = Lexer::new(&source_program);

    let mut parser = match Parser::from_lexer(lexer) {
        Ok(p) => p,
        Err(e) => {
            diagnostics.report(e);
            diagnostics.print();
            return;
        }
    };

    // Parse Program (with error recovery)
    let (ast, parse_errors) = parser.parse_translation_unit();
    diagnostics.report_all(parse_errors);

    if diagnostics.panic() {
        diagnostics.print();
        return;
    }

    // Semantic analysis (with error collection)
    let mut sem = SemanticAnalyzer::new();
    let sem_errors = sem.analyze_program(&ast);
    diagnostics.report_all(sem_errors);

    if diagnostics.panic() {
        diagnostics.print();
        return;
    }

    // Identifier Renaming
    let resolution_map = resolve_names(&ast).expect("Name resolution failed");

    // Desuger AST into IR
    let ctx = LoweringContext::new(&resolution_map);
    let mut ir = ctx.lower_program(&ast);

    // Optimization passes if optimizations are enabled
    if cli.opt {
        ir.optimize();
    }

    let mut output = String::new();
    // Print AST to console
    if cli.printast {
        let mut ast_printer = AstPrinter::new();

        println!("=== AST ===");
        for decl in &ast {
            let _ = ast_printer.print_decl(decl, &mut output);
        }

        println!("{}", output);
        output.clear();
    }

    // Print IR to console
    if cli.printir {
        let _ = IrPrinter::print_ir(&ir, &mut output);
        println!("{}", output);
    }

    // Emit x86 assembly
    let asm = backend::pipeline::compile_program(&ir);

    // Output assembly to file
    let asm_output = format!("{}.s", cli.output_file);
    let out_path = std::path::Path::new(&asm_output);
    let asm_program = backend::emit::emit_asm(&asm);
    fs::write(out_path, asm_program).expect("Failed to write assembly to file");

    assamble_program(&cli.output_file, &asm_output);

    if !cli.asm {
        clean_up(&asm_output);
    }
}

/// Invokes GCC on the ``asm_output`` file and produces the executable.
fn assamble_program(output_path: &str, asm_output: &str) -> () {
    let _ = Command::new("gcc")
        .args(["-no-pie", "-o", output_path, asm_output])
        .output()
        .expect("Failed to link program");
}

/// Clean up fils produces during compilation.
/// Removes:
///   - ``asm_output``
fn clean_up(asm_output: &str) -> () {
    let _ = Command::new("rm")
        .arg(asm_output)
        .output()
        .expect("Failed to delete intermediete files");
}
