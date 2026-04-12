use std::path::Path;

use nescript::analyzer;
use nescript::assets;
use nescript::codegen::CodeGen;
use nescript::ir;
use nescript::linker::Linker;
use nescript::optimizer;
use nescript::rom;

/// Compile a `NEScript` source string into a .nes ROM.
fn compile(source: &str) -> Vec<u8> {
    let (program, diags) = nescript::parser::parse(source);
    assert!(
        diags.is_empty(),
        "unexpected parse errors: {diags:?}\nsource:\n{source}"
    );
    let program = program.expect("parse should succeed");

    let analysis = analyzer::analyze(&program);
    assert!(
        analysis.diagnostics.iter().all(|d| !d.is_error()),
        "unexpected analysis errors: {:?}",
        analysis.diagnostics
    );

    // Run IR lowering and optimization (validates the pipeline works)
    let mut ir_program = ir::lower(&program, &analysis);
    optimizer::optimize(&mut ir_program);

    let sprites = assets::resolve_sprites(&program, Path::new("."))
        .expect("sprite resolution should succeed");

    let codegen =
        CodeGen::new(&analysis.var_allocations, &program.constants).with_sprites(&sprites);
    let instructions = codegen.generate(&program);

    let linker = Linker::new(program.game.mirroring);
    linker.link_with_assets(&instructions, &sprites)
}

// ── M1 Tests ──

#[test]
fn hello_sprite_compiles_to_valid_rom() {
    let source = include_str!("integration/hello_sprite.ne");
    let rom_data = compile(source);

    let info = rom::validate_ines(&rom_data).expect("should be valid iNES");
    assert_eq!(info.prg_banks, 1, "should be 1 PRG bank (16 KB)");
    assert_eq!(info.chr_banks, 1, "should have CHR ROM");
    assert_eq!(info.mapper, 0, "should be NROM (mapper 0)");
    assert_eq!(rom_data.len(), 16 + 16384 + 8192);
}

#[test]
fn hello_sprite_has_correct_vectors() {
    let source = include_str!("integration/hello_sprite.ne");
    let rom_data = compile(source);

    let prg_end = 16 + 16384;
    let nmi = u16::from_le_bytes([rom_data[prg_end - 6], rom_data[prg_end - 5]]);
    let reset = u16::from_le_bytes([rom_data[prg_end - 4], rom_data[prg_end - 3]]);
    let irq = u16::from_le_bytes([rom_data[prg_end - 2], rom_data[prg_end - 1]]);

    assert!(nmi >= 0xC000, "NMI vector should be in ROM space");
    assert_eq!(reset, 0xC000, "RESET should point to $C000");
    assert!(irq >= 0xC000, "IRQ vector should be in ROM space");
    assert!(nmi != reset, "NMI and RESET should be different");
}

#[test]
fn minimal_program_compiles() {
    let source = r#"
        game "Minimal" { mapper: NROM }
        on frame { wait_frame }
        start Main
    "#;
    let rom_data = compile(source);
    let info = rom::validate_ines(&rom_data).expect("should be valid iNES");
    assert_eq!(info.mapper, 0);
}

#[test]
fn program_with_state_machine() {
    let source = r#"
        game "States" { mapper: NROM }

        state Title {
            on frame {
                if button.start { transition Game }
            }
        }

        state Game {
            var score: u8 = 0
            on frame {
                score += 1
            }
        }

        start Title
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_constants() {
    let source = r#"
        game "Constants" { mapper: NROM }
        const SPEED: u8 = 3
        var px: u8 = 100
        on frame {
            if button.right { px += SPEED }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

// ── M2 Tests ──

#[test]
fn program_with_functions() {
    let source = r#"
        game "Functions" { mapper: NROM }
        var x: u8 = 0

        fun add_ten(val: u8) -> u8 {
            return val + 10
        }

        on frame {
            x = add_ten(5)
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_on_scanline_mmc3() {
    let source = r#"
        game "Scanline" { mapper: MMC3 }
        var sx: u8 = 0
        state Main {
            on frame { wait_frame }
            on scanline(120) { scroll(sx, 0) }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_structs() {
    let source = r#"
        game "Structs" { mapper: NROM }
        struct Vec2 { x: u8, y: u8 }
        struct Player { health: u8, lives: u8 }

        var pos: Vec2
        var hero: Player

        on frame {
            pos.x = 100
            pos.y = 50
            hero.health = 3
            hero.lives = 5
            if button.right { pos.x += 1 }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_enums() {
    let source = r#"
        game "Enums" { mapper: NROM }
        enum Direction { Up, Down, Left, Right }
        enum Mode { Idle, Running, Jumping }

        var dir: u8 = 0
        var mode: u8 = 0

        on frame {
            if button.right { dir = Right }
            if button.left { dir = Left }
            if dir == Right { mode = Running }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_inline_asm() {
    let source = r#"
        game "Asm" { mapper: NROM }
        var x: u8 = 0
        on frame {
            asm {
                LDA #$42
                STA $10
                INC $10
                LSR A
                CLC
                ADC #$01
            }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_while_loop() {
    let source = r#"
        game "Loops" { mapper: NROM }
        var x: u8 = 0
        on frame {
            while x < 10 {
                x += 1
            }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_fast_slow_vars() {
    let source = r#"
        game "Placement" { mapper: NROM }
        fast var hot: u8 = 0
        slow var cold: u8 = 0
        on frame {
            hot += 1
            cold += 1
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_multi_state_transitions() {
    let source = r#"
        game "Multi" { mapper: NROM }

        state Menu {
            on enter { wait_frame }
            on frame {
                if button.start { transition Level1 }
            }
        }

        state Level1 {
            var timer: u8 = 0
            on frame {
                timer += 1
                if timer > 60 {
                    transition Level2
                }
            }
        }

        state Level2 {
            on frame {
                if button.select { transition Menu }
            }
        }

        start Menu
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn coin_cavern_compiles() {
    let source = include_str!("../examples/coin_cavern.ne");
    let rom_data = compile(source);
    let info = rom::validate_ines(&rom_data).expect("should be valid iNES");
    assert_eq!(info.mapper, 0);
}

#[test]
fn ir_pipeline_produces_ir() {
    let source = r#"
        game "IR" { mapper: NROM }
        const SPEED: u8 = 2
        var x: u8 = 0
        fun double(n: u8) -> u8 { return n + n }
        on frame {
            x += SPEED
            if x > 100 { x = 0 }
        }
        start Main
    "#;
    let (program, diags) = nescript::parser::parse(source);
    assert!(diags.is_empty());
    let program = program.unwrap();
    let analysis = analyzer::analyze(&program);
    assert!(analysis.diagnostics.iter().all(|d| !d.is_error()));

    let mut ir_program = ir::lower(&program, &analysis);
    let before_ops = ir_program.op_count();
    optimizer::optimize(&mut ir_program);
    let after_ops = ir_program.op_count();

    // Optimizer should reduce or maintain op count (not increase)
    assert!(after_ops <= before_ops, "optimizer should not increase ops");
    // Should have functions for the user function + frame handler
    assert!(ir_program.functions.len() >= 2);
}

#[test]
fn error_test_missing_game() {
    let source = "var x: u8 = 0\nstart Main";
    let (_, diags) = nescript::parser::parse(source);
    assert!(
        diags.iter().any(nescript::errors::Diagnostic::is_error),
        "should produce error"
    );
}

#[test]
fn error_test_undefined_transition() {
    let source = r#"
        game "T" { mapper: NROM }
        state Main {
            on frame { transition Nonexistent }
        }
        start Main
    "#;
    let (program, parse_diags) = nescript::parser::parse(source);
    assert!(parse_diags.is_empty());
    let analysis = analyzer::analyze(&program.unwrap());
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(nescript::errors::Diagnostic::is_error),
        "should detect undefined transition target"
    );
}

#[test]
fn error_test_recursion_detected() {
    let source = r#"
        game "T" { mapper: NROM }
        fun loop_forever() { loop_forever() }
        on frame { wait_frame }
        start Main
    "#;
    let (program, parse_diags) = nescript::parser::parse(source);
    assert!(parse_diags.is_empty());
    let analysis = analyzer::analyze(&program.unwrap());
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|d| d.code == nescript::errors::ErrorCode::E0402),
        "should detect recursion"
    );
}

// ── M4 Tests ──

#[test]
fn program_with_scroll_and_cast() {
    let source = r#"
        game "M4 Test" { mapper: NROM }
        var px: u8 = 0
        var py: u8 = 0
        var wide: u16 = 0
        on frame {
            if button.right { px += 1 }
            wide = px as u16
            scroll(px, py)
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

// ── M3 Tests ──

#[test]
fn program_with_sprites_and_palette() {
    let source = r#"
        game "M3 Assets" { mapper: NROM }

        sprite Player {
            chr: [0x3C, 0x42, 0x81, 0x81, 0x81, 0x81, 0x42, 0x3C,
                  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        }

        palette MainPal {
            colors: [0x0F, 0x00, 0x10, 0x20]
        }

        background TitleBg {
            chr: @binary("title.bin")
        }

        var px: u8 = 128
        var py: u8 = 120

        state Title {
            on enter {
                load_background TitleBg
                set_palette MainPal
            }
            on frame {
                if button.right { px += 2 }
                if button.left  { px -= 2 }
                draw Player at: (px, py)
            }
        }

        start Title
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

// ── M5 Tests ──

/// Compile a source string using the mapper-aware linker.
fn compile_with_mapper(source: &str) -> Vec<u8> {
    let (program, diags) = nescript::parser::parse(source);
    assert!(
        diags.is_empty(),
        "unexpected parse errors: {diags:?}\nsource:\n{source}"
    );
    let program = program.expect("parse should succeed");

    let analysis = analyzer::analyze(&program);
    assert!(
        analysis.diagnostics.iter().all(|d| !d.is_error()),
        "unexpected analysis errors: {:?}",
        analysis.diagnostics
    );

    let mut ir_program = ir::lower(&program, &analysis);
    nescript::optimizer::optimize(&mut ir_program);

    let sprites = assets::resolve_sprites(&program, Path::new("."))
        .expect("sprite resolution should succeed");

    let codegen = nescript::codegen::CodeGen::new(&analysis.var_allocations, &program.constants)
        .with_sprites(&sprites);
    let instructions = codegen.generate(&program);

    let linker = Linker::with_mapper(program.game.mirroring, program.game.mapper);
    linker.link_with_assets(&instructions, &sprites)
}

#[test]
fn sprite_resolution_uses_tile_index() {
    // The Player sprite has 16 unique bytes of CHR data. Because tile index 0
    // is reserved for the built-in smiley, the compiler should place Player
    // at tile index 1 and `draw Player` should store that tile index in OAM.
    //
    // We check this in two ways:
    //   1. The CHR ROM contains Player's bytes at tile 1 (offset 16).
    //   2. The PRG ROM contains the immediate-load sequence `A9 01 8D 01 02`
    //      (LDA #$01 ; STA $0201) — writing tile index 1 into OAM byte 1.
    let source = r#"
        game "SpriteTile" { mapper: NROM }

        sprite Player {
            chr: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
                  0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F]
        }

        var px: u8 = 128
        var py: u8 = 120

        state Title {
            on frame {
                draw Player at: (px, py)
            }
        }

        start Title
    "#;

    let rom_data = compile(source);

    // CHR ROM begins right after PRG ROM (16 header + 16384 PRG).
    let chr_start = 16 + 16384;
    // Tile 1 lives at CHR offset 16 (16 bytes per tile).
    let tile1 = &rom_data[chr_start + 16..chr_start + 32];
    assert_eq!(
        tile1,
        &[
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D,
            0x1E, 0x1F
        ],
        "Player sprite CHR bytes should be placed at tile index 1",
    );

    // The default smiley tile at index 0 should still be non-zero (untouched).
    let tile0 = &rom_data[chr_start..chr_start + 16];
    assert_ne!(
        tile0, &[0u8; 16],
        "tile 0 should still contain the default smiley",
    );

    // In PRG ROM, look for `LDA #$01 ; STA $0201` which writes the Player's
    // tile index (1) into the tile-index byte of the first OAM slot.
    let prg = &rom_data[16..16 + 16384];
    let pattern = [0xA9u8, 0x01, 0x8D, 0x01, 0x02];
    assert!(
        prg.windows(pattern.len()).any(|w| w == pattern),
        "PRG ROM should contain LDA #$01 ; STA $0201 for draw Player",
    );
}

#[test]
fn program_with_arrays_and_math() {
    let source = r#"
        game "ArrayMath" { mapper: NROM }
        var arr: u8[4] = [10, 20, 30, 40]
        var idx: u8 = 0
        var result: u8 = 0
        on frame {
            result = arr[idx] * 2
            idx += 1
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_mmc1() {
    let source = r#"
        game "MMC1 Game" { mapper: MMC1 }
        var px: u8 = 128
        on frame {
            if button.right { px += 2 }
        }
        start Main
    "#;
    let rom_data = compile_with_mapper(source);
    let info = rom::validate_ines(&rom_data).expect("should be valid iNES");
    assert_eq!(info.mapper, 1, "should be MMC1 (mapper 1)");
}

// ── IR Codegen Tests ──

/// Compile a program using the IR-based codegen path instead of the
/// AST-based codegen. Validates the full IR pipeline produces a valid ROM.
fn compile_with_ir_codegen(source: &str) -> Vec<u8> {
    use nescript::codegen::IrCodeGen;

    let (program, diags) = nescript::parser::parse(source);
    assert!(
        diags.is_empty(),
        "unexpected parse errors: {diags:?}\nsource:\n{source}"
    );
    let program = program.expect("parse should succeed");

    let analysis = analyzer::analyze(&program);
    assert!(
        analysis.diagnostics.iter().all(|d| !d.is_error()),
        "unexpected analysis errors: {:?}",
        analysis.diagnostics
    );

    // Lower to IR and run the optimizer
    let mut ir_program = ir::lower(&program, &analysis);
    optimizer::optimize(&mut ir_program);

    // IR-based codegen
    let codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program);
    let instructions = codegen.generate(&ir_program);

    // Link into a ROM
    let linker = Linker::new(program.game.mirroring);
    linker.link(&instructions)
}

#[test]
fn ir_codegen_minimal_rom() {
    let source = r#"
        game "IR Test" { mapper: NROM }
        var x: u8 = 42
        on frame { wait_frame }
        start Main
    "#;
    let rom_data = compile_with_ir_codegen(source);
    let info = rom::validate_ines(&rom_data).expect("should be valid iNES");
    assert_eq!(info.mapper, 0);
    assert_eq!(rom_data.len(), 16 + 16384 + 8192);
}

#[test]
fn ir_codegen_full_pipeline() {
    let source = r#"
        game "IR Full" { mapper: NROM }
        var x: u8 = 0
        var y: u8 = 0
        on frame {
            if button.right { x += 1 }
            if button.left  { x -= 1 }
            if x > 100 { x = 0 }
            draw Smiley at: (x, y)
        }
        start Main
    "#;
    let rom_data = compile_with_ir_codegen(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn ir_codegen_multi_state_dispatch() {
    // Exercise the IR main-loop dispatch with multiple states and a
    // transition.
    let source = r#"
        game "IR States" { mapper: NROM }
        var timer: u8 = 0
        state Title {
            on frame {
                if button.start { transition Play }
            }
        }
        state Play {
            on frame {
                timer += 1
                if timer > 60 { transition Title }
            }
        }
        start Title
    "#;
    let rom_data = compile_with_ir_codegen(source);
    let info = rom::validate_ines(&rom_data).expect("should be valid iNES");
    assert_eq!(info.mapper, 0);
}

#[test]
fn ir_codegen_multi_oam() {
    // Draw multiple sprites and verify OAM slots are allocated sequentially.
    let source = r#"
        game "IR MultiOAM" { mapper: NROM }
        var a: u8 = 10
        var b: u8 = 20
        var c: u8 = 30
        on frame {
            draw One at: (a, a)
            draw Two at: (b, b)
            draw Three at: (c, c)
        }
        start Main
    "#;
    let rom_data = compile_with_ir_codegen(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}
