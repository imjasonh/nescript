use clap::Parser;
use std::path::{Path, PathBuf};

use nescript::analyzer;
use nescript::assets;
use nescript::codegen::{CodeGen, IrCodeGen};
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

        /// Enable debug mode (runtime checks, debug.log)
        #[arg(long)]
        debug: bool,

        /// Dump generated 6502 assembly to stdout
        #[arg(long)]
        asm_dump: bool,

        /// Dump the lowered IR program to stdout (after optimization)
        #[arg(long)]
        dump_ir: bool,

        /// Use the legacy AST-based codegen. The default is the IR-based
        /// codegen, which runs the optimizer passes before emitting 6502.
        #[arg(long)]
        use_ast: bool,
    },
    /// Type-check a source file without building
    Check {
        /// Input source file
        input: PathBuf,
    },
    /// Run all *_test.ne files in a directory (always compiled with debug mode)
    Test {
        /// Directory to search for test files (default: current directory)
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli {
        Cli::Build {
            input,
            output,
            debug,
            asm_dump,
            dump_ir,
            use_ast,
        } => {
            let output = output.unwrap_or_else(|| input.with_extension("nes"));
            match compile(
                &input,
                &CompileOptions {
                    debug,
                    asm_dump,
                    dump_ir,
                    use_ast,
                },
            ) {
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
        Cli::Test { dir } => {
            if !run_tests(&dir) {
                std::process::exit(1);
            }
        }
    }
}

fn dump_asm(instructions: &[nescript::asm::Instruction]) {
    for inst in instructions {
        if let nescript::asm::AddressingMode::Label(name) = &inst.mode {
            println!("{name}:");
            continue;
        }
        println!("    {:?} {:?}", inst.opcode, inst.mode);
    }
}

#[allow(clippy::struct_excessive_bools)]
struct CompileOptions {
    debug: bool,
    asm_dump: bool,
    dump_ir: bool,
    use_ast: bool,
}

fn compile(input: &PathBuf, opts: &CompileOptions) -> Result<Vec<u8>, ()> {
    let debug = opts.debug;
    let asm_dump = opts.asm_dump;
    let dump_ir = opts.dump_ir;
    let use_ast = opts.use_ast;
    let raw_source = std::fs::read_to_string(input).map_err(|e| {
        eprintln!("error: failed to read {}: {e}", input.display());
    })?;

    // Preprocess: inline include directives
    let source = nescript::parser::preprocess_source(&raw_source, Some(input)).map_err(|e| {
        eprintln!("error: {e}");
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

    if dump_ir {
        print!("{}", ir_program.pretty());
    }

    // Resolve sprite assets (CHR data + tile indices) relative to the
    // source file's directory, so `@binary` / `@chr` paths work naturally.
    let source_dir = input.parent().unwrap_or_else(|| Path::new("."));
    let sprites = assets::resolve_sprites(&program, source_dir).map_err(|e| {
        eprintln!("error: {e}");
    })?;

    // Code generation: IR-based is the default. `--use-ast` switches to
    // the legacy AST-based codegen for comparison and fallback.
    let mut instructions = if use_ast {
        CodeGen::new(&analysis.var_allocations, &program.constants)
            .with_sprites(&sprites)
            .with_enums(&program.enums)
            .with_debug(debug)
            .generate(&program)
    } else {
        IrCodeGen::new(&analysis.var_allocations, &ir_program)
            .with_sprites(&sprites)
            .with_debug(debug)
            .generate(&ir_program)
    };

    // Peephole optimization: cheap pass that removes redundant
    // store-then-load pairs over IR temp slots. Biggest win for the
    // IR codegen, but safe for the AST codegen too.
    nescript::codegen::peephole::optimize(&mut instructions);

    if asm_dump {
        dump_asm(&instructions);
    }

    // Link into ROM with sprite CHR data placed at each sprite's tile index.
    let linker = Linker::new(program.game.mirroring);
    let rom = linker.link_with_assets(&instructions, &sprites);

    Ok(rom)
}

fn check(input: &PathBuf) -> Result<(), ()> {
    let raw_source = std::fs::read_to_string(input).map_err(|e| {
        eprintln!("error: failed to read {}: {e}", input.display());
    })?;

    let source = nescript::parser::preprocess_source(&raw_source, Some(input)).map_err(|e| {
        eprintln!("error: {e}");
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

/// Discover all `*_test.ne` files in `dir`, compile each in debug mode, and
/// report results. Returns `true` if all tests passed (or none were found).
fn run_tests(dir: &Path) -> bool {
    // Collect all *_test.ne files in the given directory (non-recursive).
    let mut test_files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.is_file()
                    && path.extension().is_some_and(|ext| ext == "ne")
                    && path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|name| name.ends_with("_test"))
                {
                    Some(path)
                } else {
                    None
                }
            })
            .collect(),
        Err(e) => {
            eprintln!("error: failed to read directory {}: {e}", dir.display());
            return false;
        }
    };

    if test_files.is_empty() {
        println!("no test files found in {}", dir.display());
        return true;
    }

    test_files.sort();

    let total = test_files.len();
    let mut passed = 0usize;
    let mut failed = 0usize;

    println!("running {} test{}", total, if total == 1 { "" } else { "s" });

    for path in &test_files {
        let name = path.display().to_string();
        print!("test {name} ... ");

        let result = compile(
            path,
            &CompileOptions {
                debug: true,
                asm_dump: false,
                dump_ir: false,
                use_ast: false,
            },
        );

        match result {
            Ok(_) => {
                println!("ok");
                passed += 1;
            }
            Err(()) => {
                println!("FAILED");
                failed += 1;
            }
        }
    }

    println!();
    if failed == 0 {
        println!("test result: ok. {passed} passed; 0 failed");
        true
    } else {
        println!("test result: FAILED. {passed} passed; {failed} failed");
        false
    }
}
