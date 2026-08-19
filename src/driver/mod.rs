//! Compiler Driver.

use std::{fs, path::Path, process::Command, process::ExitCode};

use cli::get_arguments;
use diagnostics::Diagnostics;

use crate::{
    backend,
    frontend::{lexer::Lexer, parser::Parser, renamer::resolve_names, semantic::SemanticAnalyzer},
    middle::desuger::{LoweringContext, ProgramIr},
    middle::ssa,
    printer::{ast_printer::AstPrinter, ir_printer::IrPrinter},
};

mod cli;
pub mod diagnostics;

/// Run the complete Compiler pipeline.
/// Also Handles command line arguments.
///
/// # Returns
///
/// [`ExitCode::SUCCESS`] when an executable (or assembly listing) was
/// produced, [`ExitCode::FAILURE`] when any diagnostic was emitted or an
/// external step such as reading the source or linking failed. Callers --
/// shells and test harnesses alike -- rely on this status to tell a rejected
/// program from a compiled one.
pub fn run() -> ExitCode {
    // Get command line arguments
    let cli = get_arguments();

    // Read source file. Diagnostics quote from it, so it is read before there
    // is anywhere to report an error to.
    let source_program = match fs::read_to_string(&cli.source_file) {
        Ok(source) => source,
        Err(e) => return fatal(&format!("cannot read '{}': {}", cli.source_file, e)),
    };
    let mut diagnostics = Diagnostics::new(&cli.source_file, &source_program);

    // Tokenize program
    let lexer = Lexer::new(&source_program);

    let mut parser = match Parser::from_lexer(lexer) {
        Ok(p) => p,
        Err(e) => {
            diagnostics.report(e);
            diagnostics.print();
            return ExitCode::FAILURE;
        }
    };

    // Parse Program (with error recovery)
    let (ast, parse_errors) = parser.parse_translation_unit();
    diagnostics.report_all(parse_errors);

    if diagnostics.panic() {
        diagnostics.print();
        return ExitCode::FAILURE;
    }

    // Semantic analysis (with error collection)
    let mut sem = SemanticAnalyzer::new();
    let sem_errors = sem.analyze_program(&ast);
    diagnostics.report_all(sem_errors);

    if diagnostics.panic() {
        diagnostics.print();
        return ExitCode::FAILURE;
    }

    // Identifier Renaming.
    // Semantic analysis already rejects the programs this pass can reject, so
    // a failure here means the two passes disagree -- a compiler bug, not a
    // user error.
    let resolution_map = match resolve_names(&ast) {
        Ok(map) => map,
        Err(e) => {
            return fatal(&format!("internal compiler error: name resolution: {e}"));
        }
    };

    // Desuger AST into IR
    let ctx = LoweringContext::new(&resolution_map);
    let mut ir = ctx.lower_program(&ast);

    // Rebuild the middle-end in SSA form, and optimise it there when asked.
    // Construction and destruction run at every optimisation level, so that
    // every test exercises them.
    optimize_through_ssa(&mut ir, cli.opt);

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

    // Select instructions, allocate registers and emit assembly
    let assembly = backend::compile_to_assembly(&ir);

    // Output assembly to file
    let asm_output = format!("{}.s", cli.output_file);
    let out_path = Path::new(&asm_output);
    if let Err(e) = fs::write(out_path, assembly) {
        return fatal(&format!("cannot write '{}': {}", asm_output, e));
    }

    if let Err(message) = assamble_program(&cli.output_file, &asm_output) {
        return fatal(&message);
    }

    if !cli.asm {
        clean_up(&asm_output);
    }

    ExitCode::SUCCESS
}

/// Rebuild every function by way of SSA form.
///
/// The result is the same program: construction and destruction are inverses,
/// and no pass runs in between.  What it buys is that every test exercises the
/// two, which is what has to be true before anything optimises on SSA.
///
/// It runs whether or not optimisation was asked for.  Doing it at both levels
/// doubles the number of programs that exercise construction and destruction,
/// which is the part most likely to miscompile, and keeps the two levels
/// working on the same shape of IR.
fn optimize_through_ssa(ir: &mut ProgramIr, optimize: bool) {
    // Destructured so that the two side tables can be read while the functions
    // are being replaced; borrowing `ir` whole would not allow both.
    let ProgramIr {
        functions,
        array_sizes,
        var_names,
    } = ir;

    for (name, cfg) in functions.iter_mut() {
        let mut function = ssa::build::build(name, cfg);
        function.set_variable_names(var_names.clone());
        ssa::verify::debug_assert_valid(&function, "SSA construction");

        let promotable = ssa::promote::promotable(&function, array_sizes);
        ssa::mem2reg::promote_slots(&mut function, &promotable);
        ssa::verify::debug_assert_valid(&function, "promotion to SSA values");

        if optimize {
            ssa::passes::optimize(&mut function);
        }

        *cfg = ssa::destruct::to_cfg(&function);
    }
}

/// Invokes GCC on the ``asm_output`` file and produces the executable.
///
/// # Errors
///
/// Returns a human readable message when GCC cannot be spawned or exits with
/// a non-zero status; GCC's own stderr is forwarded so the assembler's
/// complaint is not swallowed.
fn assamble_program(output_path: &str, asm_output: &str) -> Result<(), String> {
    let output = Command::new("gcc")
        .args(["-no-pie", "-o", output_path, asm_output])
        .output()
        .map_err(|e| format!("failed to run gcc: {}", e))?;

    if !output.status.success() {
        // `from_utf8_lossy` avoids failing on a non-UTF-8 message from gcc.
        return Err(format!(
            "gcc failed to assemble '{}':\n{}",
            asm_output,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

/// Clean up fils produces during compilation.
/// Removes:
///   - ``asm_output``
fn clean_up(asm_output: &str) -> () {
    // A failed cleanup leaves a stray listing behind but does not invalidate
    // the executable, so it is reported without failing the compilation.
    if let Err(e) = fs::remove_file(asm_output) {
        eprintln!("warning: could not remove '{}': {}", asm_output, e);
    }
}

/// Report a failure that is not tied to a source span and end the run.
fn fatal(message: &str) -> ExitCode {
    eprintln!("error: {}", message);
    ExitCode::FAILURE
}
