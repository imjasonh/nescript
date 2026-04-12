use nescript::analyzer;
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

    let codegen = CodeGen::new(&analysis.var_allocations, &program.constants);
    let instructions = codegen.generate(&program);

    let linker = Linker::new(program.game.mirroring);
    linker.link(&instructions)
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

    let codegen = nescript::codegen::CodeGen::new(&analysis.var_allocations, &program.constants);
    let instructions = codegen.generate(&program);

    let linker = Linker::with_mapper(program.game.mirroring, program.game.mapper);
    linker.link(&instructions)
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
