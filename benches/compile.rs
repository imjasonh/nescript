//! End-to-end compilation benchmarks.
//!
//! Each `examples/*.ne` file becomes its own Criterion group that
//! times the full `parse → analyze → lower → optimize → codegen →
//! peephole → link` pipeline the `nescript build` CLI runs. The goal
//! is to catch compile-time regressions — today every example
//! compiles in well under 100 ms, so a change that doubles that
//! shows up as a large red bar in `cargo bench`'s output.
//!
//! The harness pre-reads every source file into memory before any
//! measurement starts. Criterion's sample iterations then run only
//! the in-memory compile path, so disk I/O never shows up on the
//! hot loop.

use std::fs;
use std::path::{Path, PathBuf};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use nescript::analyzer;
use nescript::assets;
use nescript::codegen::{peephole, IrCodeGen};
use nescript::ir;
use nescript::linker::{Linker, PrgBank};
use nescript::optimizer;
use nescript::parser;
use nescript::parser::ast::BankType;

/// Pre-loaded `.ne` source plus the directory it was read from. The
/// directory matters because sprite `@binary` / `@chr` paths resolve
/// relative to the source file — the current examples all use inline
/// CHR, but resolving relative to the right directory keeps the bench
/// honest if an example later grows an external asset.
struct Example {
    name: String,
    source: String,
    source_dir: PathBuf,
}

/// Scan `examples/*.ne` at the repo root and load every source file
/// into memory. Sorted by file name so the benchmark output is
/// reproducible across runs.
fn load_examples() -> Vec<Example> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let examples_dir = manifest_dir.join("examples");

    let mut entries: Vec<PathBuf> = fs::read_dir(&examples_dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", examples_dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ne"))
        .collect();
    entries.sort();

    entries
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let source_dir = path
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            Example {
                name,
                source,
                source_dir,
            }
        })
        .collect()
}

/// Run the full CLI compile pipeline on an in-memory source string.
/// Mirrors `compile` in `src/main.rs`: parse → analyze → IR lower →
/// optimize → IR codegen → peephole → link. Panics on any error so
/// a regression that breaks the pipeline surfaces immediately instead
/// of silently skewing the measurements.
fn compile_pipeline(source: &str, source_dir: &Path) -> Vec<u8> {
    let preprocessed = parser::preprocess_source(source, None)
        .unwrap_or_else(|e| panic!("preprocess failed: {e}"));

    let (program, parse_diags) = parser::parse(&preprocessed);
    assert!(
        !parse_diags
            .iter()
            .any(nescript::errors::Diagnostic::is_error),
        "parse errors: {parse_diags:?}"
    );
    let program = program.expect("parse produced no program");

    let analysis = analyzer::analyze(&program);
    assert!(
        !analysis
            .diagnostics
            .iter()
            .any(nescript::errors::Diagnostic::is_error),
        "analysis errors: {:?}",
        analysis.diagnostics
    );

    let mut ir_program = ir::lower(&program, &analysis);
    optimizer::optimize(&mut ir_program);

    let sprites = assets::resolve_sprites(&program, source_dir).expect("sprite resolution failed");
    let sfx = assets::resolve_sfx(&program).expect("sfx resolution failed");
    let music = assets::resolve_music(&program).expect("music resolution failed");
    let palettes = assets::resolve_palettes(&program);
    let backgrounds = assets::resolve_backgrounds(&program);

    let mut instructions = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music)
        .generate(&ir_program);
    peephole::optimize(&mut instructions);

    let linker = Linker::with_mapper(program.game.mirroring, program.game.mapper);
    let switchable_banks: Vec<PrgBank> = program
        .banks
        .iter()
        .filter(|b| b.bank_type == BankType::Prg)
        .map(|b| PrgBank::empty(&b.name))
        .collect();
    linker.link_banked_with_ppu(
        &instructions,
        &sprites,
        &sfx,
        &music,
        &palettes,
        &backgrounds,
        &switchable_banks,
    )
}

/// Criterion entry point. One benchmark group per example so the
/// HTML report groups them sensibly and so individual regressions
/// are easy to spot.
fn bench_compile(c: &mut Criterion) {
    let examples = load_examples();
    assert!(
        !examples.is_empty(),
        "no examples found under examples/*.ne — benchmark would measure nothing"
    );

    for example in &examples {
        let mut group = c.benchmark_group(format!("compile/{}", example.name));
        group.bench_with_input(
            BenchmarkId::from_parameter(&example.name),
            example,
            |b, ex| {
                b.iter(|| compile_pipeline(&ex.source, &ex.source_dir));
            },
        );
        group.finish();
    }
}

criterion_group!(benches, bench_compile);
criterion_main!(benches);
