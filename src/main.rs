use clap::Parser;
use std::path::PathBuf;

use nescript::analyzer;
use nescript::codegen::CodeGen;
use nescript::errors::render_diagnostics;
use nescript::ir;
use nescript::linker::Linker;
use nescript::optimizer;

#[derive(Parser)]
#[command(name = "nescript", about = "NEScript compiler — NES game development")]
enum Cli {
    /// Compile a .ne source file into a .nes ROM
    Build {
        /// Input source file
        input: PathBuf,

        /// Output ROM file (default: input with .nes extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Type-check a source file without building
    Check {
        /// Input source file
        input: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli {
        Cli::Build { input, output } => {
            let output = output.unwrap_or_else(|| input.with_extension("nes"));
            match compile(&input) {
                Ok(rom) => {
                    std::fs::write(&output, rom).unwrap_or_else(|e| {
                        eprintln!("error: failed to write {}: {e}", output.display());
                        std::process::exit(1);
                    });
                    println!(
                        "compiled {} -> {} ({} bytes)",
                        input.display(),
                        output.display(),
                        std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0)
                    );
                }
                Err(()) => std::process::exit(1),
            }
        }
        Cli::Check { input } => match check(&input) {
            Ok(()) => println!("no errors found in {}", input.display()),
            Err(()) => std::process::exit(1),
        },
    }
}

fn compile(input: &PathBuf) -> Result<Vec<u8>, ()> {
    let source = std::fs::read_to_string(input).map_err(|e| {
        eprintln!("error: failed to read {}: {e}", input.display());
    })?;

    let filename = input.to_string_lossy();

    // Parse
    let (program, parse_diags) = nescript::parser::parse(&source);
    if !parse_diags.is_empty() {
        render_diagnostics(&source, &filename, &parse_diags);
    }
    if parse_diags
        .iter()
        .any(nescript::errors::Diagnostic::is_error)
    {
        return Err(());
    }
    let program = program.ok_or(())?;

    // Analyze
    let analysis = analyzer::analyze(&program);
    if !analysis.diagnostics.is_empty() {
        render_diagnostics(&source, &filename, &analysis.diagnostics);
    }
    if analysis
        .diagnostics
        .iter()
        .any(nescript::errors::Diagnostic::is_error)
    {
        return Err(());
    }

    // IR lowering and optimization
    let mut ir_program = ir::lower(&program, &analysis);
    optimizer::optimize(&mut ir_program);

    // Code generation (still AST-based for M2; IR codegen comes in M3)
    let codegen = CodeGen::new(&analysis.var_allocations, &program.constants);
    let instructions = codegen.generate(&program);

    // Link into ROM
    let linker = Linker::new(program.game.mirroring);
    let rom = linker.link(&instructions);

    Ok(rom)
}

fn check(input: &PathBuf) -> Result<(), ()> {
    let source = std::fs::read_to_string(input).map_err(|e| {
        eprintln!("error: failed to read {}: {e}", input.display());
    })?;

    let filename = input.to_string_lossy();

    let (program, parse_diags) = nescript::parser::parse(&source);
    if !parse_diags.is_empty() {
        render_diagnostics(&source, &filename, &parse_diags);
    }
    if parse_diags
        .iter()
        .any(nescript::errors::Diagnostic::is_error)
    {
        return Err(());
    }
    let program = program.ok_or(())?;

    let analysis = analyzer::analyze(&program);
    if !analysis.diagnostics.is_empty() {
        render_diagnostics(&source, &filename, &analysis.diagnostics);
    }
    if analysis
        .diagnostics
        .iter()
        .any(nescript::errors::Diagnostic::is_error)
    {
        return Err(());
    }

    Ok(())
}
