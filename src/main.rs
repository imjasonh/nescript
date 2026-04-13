use clap::Parser;
use std::path::{Path, PathBuf};

use nescript::analyzer;
use nescript::assets;
use nescript::codegen::IrCodeGen;
use nescript::errors::render_diagnostics;
use nescript::ir;
use nescript::linker::{Linker, PrgBank};
use nescript::optimizer;
use nescript::parser::ast::BankType;

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

        /// Dump a human-readable memory map of variable allocations
        /// to stdout.
        #[arg(long)]
        memory_map: bool,

        /// Dump a call graph showing which functions call which.
        #[arg(long)]
        call_graph: bool,
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
        Cli::Build {
            input,
            output,
            debug,
            asm_dump,
            dump_ir,
            memory_map,
            call_graph,
        } => {
            let output = output.unwrap_or_else(|| input.with_extension("nes"));
            match compile(
                &input,
                &CompileOptions {
                    debug,
                    asm_dump,
                    dump_ir,
                    memory_map,
                    call_graph,
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
    }
}

/// Print a human-readable memory map of variable allocations.
/// Entries are sorted by address and labelled with their scope
/// (zero-page vs RAM).
fn print_memory_map(analysis: &nescript::analyzer::AnalysisResult) {
    let mut allocs: Vec<_> = analysis.var_allocations.iter().collect();
    allocs.sort_by_key(|a| a.address);

    println!("=== NEScript Memory Map ===");
    println!("Zero Page ($00-$FF):");
    println!("  $00-$0F  [SYSTEM]  reserved (frame flag, input, state, params, scratch)");
    for a in allocs.iter().filter(|a| a.address < 0x100) {
        if a.size == 1 {
            println!("  ${:04X}    [USER]    {} (u8)", a.address, a.name);
        } else {
            println!(
                "  ${:04X}-${:04X}  [USER]  {} ({} bytes)",
                a.address,
                a.address + a.size - 1,
                a.name,
                a.size
            );
        }
    }

    let ram_allocs: Vec<_> = allocs.iter().filter(|a| a.address >= 0x100).collect();
    if !ram_allocs.is_empty() {
        println!("\nRAM ($0200-$07FF):");
        println!("  $0200-$02FF  [SYSTEM]  OAM shadow buffer");
        for a in &ram_allocs {
            if a.size == 1 {
                println!("  ${:04X}        [USER]    {} (u8)", a.address, a.name);
            } else {
                println!(
                    "  ${:04X}-${:04X}  [USER]  {} ({} bytes)",
                    a.address,
                    a.address + a.size - 1,
                    a.name,
                    a.size
                );
            }
        }
    }

    // Summary line.
    let zp_used: u16 = allocs
        .iter()
        .filter(|a| a.address < 0x80)
        .map(|a| a.size)
        .sum();
    let ram_used: u16 = allocs
        .iter()
        .filter(|a| a.address >= 0x300)
        .map(|a| a.size)
        .sum();
    println!();
    println!("Zero Page: {zp_used}/128 bytes used");
    println!("Main RAM:  {ram_used}/1280 bytes used");
}

/// Print a human-readable call graph of the analyzed program.
/// Entries show the max call depth reached from each entry point
/// (state handler) and the transitive callees.
fn print_call_graph(analysis: &nescript::analyzer::AnalysisResult) {
    use std::collections::BTreeMap;

    let sorted: BTreeMap<_, _> = analysis
        .call_graph
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let max_depth = analysis.max_depths.values().copied().max().unwrap_or(0);

    println!("=== Call Graph (max depth: {max_depth} / 8) ===");
    if sorted.is_empty() {
        println!("  (no functions or handlers)");
        return;
    }
    for (caller, callees) in &sorted {
        if let Some(depth) = analysis.max_depths.get(caller) {
            println!("{caller} (max depth {depth})");
        } else {
            println!("{caller}");
        }
        if callees.is_empty() {
            println!("  └── (leaf)");
        } else {
            let count = callees.len();
            for (i, callee) in callees.iter().enumerate() {
                let branch = if i + 1 == count {
                    "└──"
                } else {
                    "├──"
                };
                println!("  {branch} {callee}");
            }
        }
    }
}

fn dump_asm(instructions: &[nescript::asm::Instruction]) {
    use nescript::asm::{AddressingMode, Opcode};
    for inst in instructions {
        // A bare `NOP` with a `Label` operand is a label *definition*
        // (the pseudo-instruction the codegen emits when marking a
        // position). Any other opcode with `Label` mode is an actual
        // instruction like `JSR foo` or `JMP bar`, so we show the
        // opcode + target.
        if inst.opcode == Opcode::NOP {
            if let AddressingMode::Label(name) = &inst.mode {
                println!("{name}:");
                continue;
            }
        }
        println!("    {:?} {:?}", inst.opcode, inst.mode);
    }
}

#[allow(clippy::struct_excessive_bools)]
struct CompileOptions {
    debug: bool,
    asm_dump: bool,
    dump_ir: bool,
    memory_map: bool,
    call_graph: bool,
}

fn compile(input: &PathBuf, opts: &CompileOptions) -> Result<Vec<u8>, ()> {
    let debug = opts.debug;
    let asm_dump = opts.asm_dump;
    let dump_ir = opts.dump_ir;
    let memory_map = opts.memory_map;
    let call_graph = opts.call_graph;
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

    if memory_map {
        print_memory_map(&analysis);
    }

    if call_graph {
        print_call_graph(&analysis);
    }

    // Resolve sprite assets (CHR data + tile indices) relative to the
    // source file's directory, so `@binary` / `@chr` paths work naturally.
    let source_dir = input.parent().unwrap_or_else(|| Path::new("."));
    let sprites = assets::resolve_sprites(&program, source_dir).map_err(|e| {
        eprintln!("error: {e}");
    })?;

    // Resolve audio assets: user-declared sfx/music plus any
    // builtins referenced via `play foo` / `start_music bar` for
    // names that aren't in the program's sfx/music declarations.
    let sfx = assets::resolve_sfx(&program).map_err(|e| {
        eprintln!("error: {e}");
    })?;
    let music = assets::resolve_music(&program).map_err(|e| {
        eprintln!("error: {e}");
    })?;

    // IR-based code generation. Lower → optimize → emit 6502.
    let mut instructions = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music)
        .with_debug(debug)
        .generate(&ir_program);

    // Peephole pass: cleans up the IR codegen's temp-heavy output —
    // dead stores, redundant loads, short-branch folds, etc.
    nescript::codegen::peephole::optimize(&mut instructions);

    if asm_dump {
        dump_asm(&instructions);
    }

    // Link into ROM with both graphic assets (sprite CHR) and audio
    // assets (sfx envelopes, music note streams) spliced in. We use
    // `Linker::with_mapper` so the iNES header's mapper byte
    // reflects the source's `mapper:` declaration — without this
    // the CLI always shipped mapper 0 (NROM) regardless of whether
    // the program actually needed MMC1/MMC3 bank switching.
    //
    // For banked mappers (MMC1, UxROM, MMC3) we collect the
    // declared `bank X: prg` entries and turn each into an empty
    // 16 KB switchable slot. User code currently still lives in
    // the fixed bank — the declared banks exist so programs that
    // outgrow 16 KB have real ROM space to grow into and so
    // mapper-specific fixtures (vectors, trampolines, bank-select
    // helpers) land in the right place.
    let linker = Linker::with_mapper(program.game.mirroring, program.game.mapper);
    let switchable_banks: Vec<PrgBank> = program
        .banks
        .iter()
        .filter(|b| b.bank_type == BankType::Prg)
        .map(|b| PrgBank::empty(&b.name))
        .collect();
    let rom = linker.link_banked(&instructions, &sprites, &sfx, &music, &switchable_banks);

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
