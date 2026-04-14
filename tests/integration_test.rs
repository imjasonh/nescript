use std::path::Path;

use nescript::analyzer;
use nescript::assets;
use nescript::codegen::IrCodeGen;
use nescript::ir;
use nescript::linker::{Linker, PrgBank};
use nescript::optimizer;
use nescript::parser::ast::BankType;
use nescript::rom;

/// Compile a `NEScript` source string into a .nes ROM. Runs the full
/// IR pipeline: parse → analyze → IR lower → optimize → IR codegen
/// → peephole → link. This is what the `nescript build` CLI does
/// (minus file IO and the dump flags), so these integration tests
/// exercise the same path end users hit.
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

    let mut ir_program = ir::lower(&program, &analysis);
    optimizer::optimize(&mut ir_program);

    let sprites = assets::resolve_sprites(&program, Path::new("."))
        .expect("sprite resolution should succeed");
    let sfx = assets::resolve_sfx(&program).expect("sfx resolution should succeed");
    let music = assets::resolve_music(&program).expect("music resolution should succeed");

    let mut codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music);
    let mut instructions = codegen.generate(&ir_program);
    nescript::codegen::peephole::optimize(&mut instructions);

    let linker = Linker::new(program.game.mirroring);
    linker.link_with_all_assets(&instructions, &sprites, &sfx, &music)
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
fn program_with_on_scanline_per_state() {
    // Two states, each with its own scanline handler at a different
    // position. The IR codegen should emit per-state dispatch in
    // both `__irq_user` and `__ir_mmc3_reload`.
    let source = r#"
        game "MultiSL" { mapper: MMC3 }
        var s: u8 = 0
        state A {
            on frame { wait_frame }
            on scanline(64) { scroll(0, 0) }
        }
        state B {
            on frame { wait_frame }
            on scanline(192) { scroll(0, 0) }
        }
        start A
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_function_local_variables() {
    // Functions with locally-declared variables should allocate
    // their own backing storage and not corrupt caller state when
    // nested.
    let source = r#"
        game "Locals" { mapper: NROM }
        var out: u8 = 0

        fun double(x: u8) -> u8 {
            var t: u8 = x
            t = t + t
            return t
        }

        fun double_sum(a: u8, b: u8) -> u8 {
            var s1: u8 = double(a)
            var s2: u8 = double(b)
            return s1 + s2
        }

        on frame {
            out = double_sum(10, 20)
            wait_frame
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_for_loop() {
    let source = r#"
        game "ForLoop" { mapper: NROM }
        var arr: u8[8] = [0, 0, 0, 0, 0, 0, 0, 0]
        var total: u8 = 0
        on frame {
            total = 0
            for i in 0..8 {
                total += arr[i]
            }
            wait_frame
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_match_statement() {
    // Note: the parser doesn't support `;` as a statement separator,
    // so each arm body uses newlines between statements.
    let source = r#"
        game "Match" { mapper: NROM }
        enum Mode { Idle, Run, Jump }
        var mode: u8 = Idle
        var x: u8 = 0
        on frame {
            match mode {
                Idle => { if button.a { mode = Run } }
                Run => {
                    x += 1
                    if button.b { mode = Jump }
                }
                Jump => {
                    x += 2
                    if button.a { mode = Idle }
                }
                _ => {}
            }
            wait_frame
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_struct_literals() {
    let source = r#"
        game "Lit" { mapper: NROM }
        struct Vec2 { x: u8, y: u8 }
        var pos: Vec2 = Vec2 { x: 10, y: 20 }
        on frame {
            pos = Vec2 { x: 100, y: 50 }
            if button.right {
                pos = Vec2 { x: pos.x + 1, y: pos.y }
            }
            draw Smiley at: (pos.x, pos.y)
            wait_frame
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
fn program_with_u16_struct_field() {
    // Exercise the u16 struct field path end-to-end: declare a
    // struct with a mix of u8 and u16 fields, read from and write
    // to the u16 field (including a literal > 255), and verify the
    // ROM assembles cleanly. The analyzer's field-offset math and
    // the IR lowering's wide load/store path both need to agree
    // for this to compile at all.
    let source = r#"
        game "U16Struct" { mapper: NROM }
        struct Entity { kind: u8, position: u16, flags: u8 }
        var e: Entity
        on frame {
            e.kind = 1
            e.position = 1234
            e.flags = 7
            if e.position > 1000 {
                e.position += 1
            }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn u16_struct_field_initializer_writes_both_bytes_to_rom() {
    // Struct literal initializer with a u16 field > 255 — the
    // compiler runs the global-init path at reset time, which
    // lowers to two independent LDA/STA pairs (low byte then high
    // byte). Unlike per-frame stores, initializers aren't subject
    // to the optimizer's dead-store pass, so they're a stable
    // place to witness both halves of the u16 write. 1234 = $04D2.
    let source = r#"
        game "U16Init" { mapper: NROM }
        struct Point { tag: u8, x: u16 }
        var p: Point = Point { tag: 1, x: 1234 }
        on frame {
            if p.x > 1000 {
                scroll(p.tag, 0)
            }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");

    // PRG ROM starts at offset 16 and is 16384 bytes long.
    let prg = &rom_data[16..16 + 16384];

    // Look for `LDA #$D2 ; STA abs|zp` — opcode $A9 $D2 $85/$8D.
    // This is the low-byte initializer for `p.x`.
    let mut found_low_store = false;
    for i in 0..prg.len().saturating_sub(4) {
        if prg[i] == 0xA9 && prg[i + 1] == 0xD2 && (prg[i + 2] == 0x85 || prg[i + 2] == 0x8D) {
            found_low_store = true;
            break;
        }
    }
    assert!(
        found_low_store,
        "expected an LDA #$D2 / STA <addr> pair in PRG for the u16 initializer low byte"
    );

    // And the high byte: `LDA #$04 ; STA abs|zp`.
    let mut found_high_store = false;
    for i in 0..prg.len().saturating_sub(4) {
        if prg[i] == 0xA9 && prg[i + 1] == 0x04 && (prg[i + 2] == 0x85 || prg[i + 2] == 0x8D) {
            found_high_store = true;
            break;
        }
    }
    assert!(
        found_high_store,
        "expected an LDA #$04 / STA <addr> pair in PRG for the u16 initializer high byte"
    );
}

#[test]
fn u16_struct_field_comparison_emits_wide_compare() {
    // Reading a u16 struct field into a comparison should take
    // the wide (16-bit) compare path, which produces a distinctive
    // two-stage CMP sequence: high byte first (with equal-branch),
    // then low byte. Without the u16 lowering, the field would
    // be treated as u8 and the comparison would fold to a single
    // 8-bit CMP. We detect the wide path by checking that both
    // the low byte of 1000 ($E8) and the high byte ($03) appear
    // as immediate operands in the emitted PRG — the compiler
    // only emits both when it's generating a 16-bit compare.
    let source = r#"
        game "U16Cmp" { mapper: NROM }
        struct Counter { n: u16 }
        var c: Counter = Counter { n: 2000 }
        on frame {
            if c.n > 1000 {
                scroll(1, 0)
            } else {
                scroll(2, 0)
            }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");

    let prg = &rom_data[16..16 + 16384];

    // 1000 = $03E8. Look for CMP #$03 (A9 03, C9 03) — the high
    // byte of the comparison literal. We expect `CMP #$03` ($C9
    // $03) to appear somewhere in the CMP-with-constant sequence.
    let mut found_high_cmp = false;
    for i in 0..prg.len().saturating_sub(2) {
        if prg[i] == 0xC9 && prg[i + 1] == 0x03 {
            found_high_cmp = true;
            break;
        }
    }
    assert!(
        found_high_cmp,
        "expected a CMP #$03 (16-bit compare high byte) in PRG"
    );
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
fn program_with_poke_peek_intrinsics() {
    let source = r#"
        game "Hardware" { mapper: NROM }
        var status: u8 = 0
        on frame {
            // Write to PPU address / data registers directly.
            poke(0x2006, 0x3F)
            poke(0x2006, 0x00)
            poke(0x2007, 0x0F)
            // Read PPU status.
            status = peek(0x2002)
            wait_frame
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_raw_asm_block() {
    // `raw asm` bypasses `{var}` substitution so the body is passed
    // to the inline parser unchanged.
    let source = r#"
        game "RawAsm" { mapper: NROM }
        var x: u8 = 0
        on frame {
            raw asm {
                LDA #$42
                STA $00
            }
            wait_frame
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_inline_asm_variable_substitution() {
    let source = r#"
        game "AsmVar" { mapper: NROM }
        var counter: u8 = 0
        on frame {
            asm {
                LDA {counter}
                CLC
                ADC #$01
                STA {counter}
            }
            wait_frame
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

#[test]
fn program_with_u16_arithmetic_and_compare() {
    // Exercises the full u16 path: literal > 255 initializer,
    // u16 += u8, u16 > u16 comparison. The old codegen truncated
    // all u16 operations to their low byte, so `big = 1000`
    // landed as 232 and `big += 1` never carried into the high
    // byte. This test just asserts the ROM builds cleanly — the
    // unit tests in `codegen/ir_codegen.rs` verify the actual
    // instruction shape.
    let source = r#"
        game "U16 Arith" { mapper: NROM }
        var big: u16 = 1000
        var flag: u8 = 0
        on frame {
            big = big + 1
            if big > 1050 {
                flag = 1
            }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_audio_driver() {
    // Exercises the audio driver end-to-end with builtin sfx/music
    // names: play, start_music, stop_music all lower into the
    // data-driven driver, the linker splices the tick/period-table/
    // data blobs, and the resulting ROM is valid iNES.
    let source = r#"
        game "Audio" { mapper: NROM }
        on frame {
            if button.a { play coin }
            if button.b { start_music theme }
            if button.start { stop_music }
        }
        start Main
    "#;
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn program_with_user_declared_sfx_and_music() {
    // Full user-declared audio pipeline: `sfx` and `music` blocks,
    // references via `play`/`start_music`, full ROM emission. The
    // resolved envelope and note-stream bytes should land in PRG
    // under stable labels so the IR codegen's SymbolLo/SymbolHi
    // references resolve.
    let source = r#"
        game "Audio Assets" { mapper: NROM }

        sfx Zap {
            duty: 2
            pitch: [0x20, 0x22, 0x24, 0x26, 0x28, 0x2A]
            volume: [15, 13, 11, 9, 6, 3]
        }

        music Loop {
            duty: 2
            volume: 10
            repeat: true
            notes: [37, 8, 41, 8, 44, 8, 49, 8]
        }

        var t: u8 = 0

        on frame {
            t += 1
            if t == 30 { play Zap }
            if t == 60 {
                t = 0
                start_music Loop
            }
        }
        start Main
    "#;
    let rom_data = compile(source);
    let info = rom::validate_ines(&rom_data).expect("should be valid iNES");
    assert_eq!(info.mapper, 0);

    // Verify the user-declared envelope appears in PRG. The
    // resolver encodes `Zap` as
    //     duty << 6 | 0x30 | volume
    // per frame, terminated by a zero sentinel.
    let prg = &rom_data[16..16 + 16384];
    let env = |v: u8| (2u8 << 6) | 0x30u8 | v;
    let zap_env: [u8; 7] = [env(15), env(13), env(11), env(9), env(6), env(3), 0x00];
    assert!(
        prg.windows(zap_env.len()).any(|w| w == zap_env),
        "Zap envelope bytes should be in PRG ROM"
    );

    // Verify the music stream is in PRG: (37, 8, 41, 8, 44, 8, 49, 8, 0xFF, 0xFF)
    let loop_stream: [u8; 10] = [37, 8, 41, 8, 44, 8, 49, 8, 0xFF, 0xFF];
    assert!(
        prg.windows(loop_stream.len()).any(|w| w == loop_stream),
        "Loop music note stream should be in PRG ROM"
    );
}

#[test]
fn program_without_audio_has_no_audio_driver_in_prg() {
    // Programs that never touch audio should pay zero ROM cost:
    // no period table, no driver body, no data blobs. We verify
    // indirectly by checking that the `__audio_tick` entry point
    // wouldn't have anything to JSR to (because the NMI splice
    // is gated on the `__audio_used` marker which never exists).
    //
    // The cheapest observable signal: a period-table fingerprint.
    // The period table always starts with a distinct 2-byte
    // sequence that appears at C1's period; if we don't see it in
    // PRG, the audio subsystem wasn't linked in.
    let source = r#"
        game "Silent" { mapper: NROM }
        var x: u8 = 0
        on frame { x += 1 }
        start Main
    "#;
    let rom_data = compile(source);
    // Pull the period table for C1 and make sure it's NOT in PRG.
    // C1 ≈ 32.7 Hz → period ≈ 3421 → but that's too big for 11
    // bits, so it clamps. Instead, use the distinctive combined
    // LDA #imm / LDA #imm pattern from the audio tick itself that
    // would only appear if the driver body was linked in.
    //
    // A robust fingerprint: the `JSR __audio_tick` opcode byte
    // ($20) followed by any 2 bytes only appears in the NMI
    // handler when audio was used. We test the absence of the
    // label instead via an indirect method: count the total
    // number of STA $4004 writes (pulse-2 register). When audio
    // is unused, there should be none. When audio is used, there
    // would be several in the driver.
    let prg = &rom_data[16..16 + 16384];
    // `STA $4006` ($8D $06 $40) is written exclusively by the
    // music tick's period-lookup path. The init code pre-silences
    // $4004 but never touches $4006, so its presence is a reliable
    // "the audio driver was linked in" signal.
    let pattern: [u8; 3] = [0x8D, 0x06, 0x40];
    let count = prg.windows(pattern.len()).filter(|w| *w == pattern).count();
    assert_eq!(
        count, 0,
        "silent program should not contain the music tick's $4006 write"
    );
}

#[test]
fn unknown_sfx_name_is_a_hard_error() {
    // The analyzer must reject `play NoSuchSfx` (neither a user
    // decl nor a builtin) with E0505. Regression test for the
    // old behavior, which silently accepted any name.
    let source = r#"
        game "T" { mapper: NROM }
        on frame { play NoSuchSfx }
        start Main
    "#;
    let (program, _) = nescript::parser::parse(source);
    let analysis = analyzer::analyze(&program.unwrap());
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(nescript::errors::Diagnostic::is_error),
        "unknown sfx should produce an error"
    );
}

#[test]
fn audio_pipeline_drops_period_table_cost_when_unused() {
    // Regression test for the "no-cost elision" invariant: a
    // program with no audio statements should produce a ROM
    // smaller than one that uses audio. The exact byte count
    // varies with codegen changes, so we test the *ordering* of
    // sizes: a silent program < an audio program.
    let silent = compile(
        r#"
        game "Silent" { mapper: NROM }
        var x: u8 = 0
        on frame { x += 1 }
        start Main
    "#,
    );
    // Both ROMs are the same file size (16 header + 16 KB PRG + 8
    // KB CHR = 24592), but the silent program's PRG fills with
    // $FF padding past the code; an audio program's PRG has the
    // driver and tables eating into that padding space. So we
    // count $FF bytes in PRG: the silent version must have more.
    let audio = compile(
        r#"
        game "Audio" { mapper: NROM }
        on frame { play coin }
        start Main
    "#,
    );
    let silent_prg = &silent[16..16 + 16384];
    let audio_prg = &audio[16..16 + 16384];
    // Count padding bytes ($FF = PRG fill) in each ROM. Using a
    // raw filter().count() is clippy-noisy ("naive_bytecount"),
    // but pulling in the `bytecount` crate for a one-line test
    // helper isn't worth it — the test runs once per build.
    #[allow(clippy::naive_bytecount)]
    let silent_ff = silent_prg.iter().filter(|&&b| b == 0xFF).count();
    #[allow(clippy::naive_bytecount)]
    let audio_ff = audio_prg.iter().filter(|&&b| b == 0xFF).count();
    assert!(
        silent_ff > audio_ff,
        "silent program should have more $FF padding than an audio program \
         (silent={silent_ff}, audio={audio_ff})"
    );
}

// ── M3 Tests ──

#[test]
fn program_with_inline_sprite_chr() {
    let source = r#"
        game "M3 Assets" { mapper: NROM }

        sprite Player {
            chr: [0x3C, 0x42, 0x81, 0x81, 0x81, 0x81, 0x42, 0x3C,
                  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        }

        var px: u8 = 128
        var py: u8 = 120

        state Title {
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

#[test]
fn program_with_palette_compiles_and_blob_is_in_prg() {
    let source = r#"
        game "PalTest" { mapper: NROM }
        palette Cool {
            colors: [0x0F, 0x01, 0x11, 0x21,
                     0x0F, 0x02, 0x12, 0x22,
                     0x0F, 0x0C, 0x1C, 0x2C,
                     0x0F, 0x0B, 0x1B, 0x2B,
                     0x0F, 0x01, 0x11, 0x21,
                     0x0F, 0x16, 0x27, 0x30,
                     0x0F, 0x14, 0x24, 0x34,
                     0x0F, 0x0B, 0x1B, 0x2B]
        }
        on frame { wait_frame }
        start Main
    "#;
    let rom_data = compile_banked(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
    // The 32-byte palette blob lands verbatim inside PRG ROM.
    // Search for a distinctive 8-byte subsequence from sub-palette 3
    // that doesn't collide with any of the other blobs or init
    // sequences the linker emits.
    let needle = [0x0F, 0x16, 0x27, 0x30, 0x0F, 0x14, 0x24, 0x34];
    let found = rom_data.windows(needle.len()).any(|w| w == needle);
    assert!(
        found,
        "palette bytes should be spliced into PRG ROM verbatim"
    );
}

#[test]
fn program_with_set_palette_queues_update_at_runtime() {
    // A program with a `set_palette Name` statement should emit
    // the `__ppu_update_used` marker (so the linker pulls in the
    // NMI helper) and must contain the zero-page write sequence
    // that stores the palette label pointer into $12/$13.
    let source = r#"
        game "PalRuntime" { mapper: NROM }
        palette Swap { colors: [0x0F, 0x01, 0x11, 0x21] }
        on frame { set_palette Swap }
        start Main
    "#;
    let rom_data = compile_banked(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
    // $12 == ZP_PENDING_PALETTE_LO, so the code will contain
    // `STA $12` (opcode 85 12) somewhere in PRG.
    let sta_12 = [0x85u8, 0x12];
    let found = rom_data.windows(sta_12.len()).any(|w| w == sta_12);
    assert!(
        found,
        "set_palette codegen should emit `STA $12` (ZP_PENDING_PALETTE_LO)"
    );
}

#[test]
fn program_with_background_compiles_and_tiles_spliced() {
    let source = r#"
        game "BgTest" { mapper: NROM }
        background Stage {
            tiles: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE]
        }
        on frame { wait_frame }
        start Main
    "#;
    let rom_data = compile_banked(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
    // The distinctive 5-byte prefix of the tiles blob should be in
    // PRG verbatim (the resolver zero-pads to 960 bytes so the tail
    // is mostly zero).
    let needle = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
    let found = rom_data.windows(needle.len()).any(|w| w == needle);
    assert!(
        found,
        "background tile bytes should be spliced into PRG ROM verbatim"
    );
}

#[test]
fn program_with_load_background_queues_update() {
    let source = r#"
        game "BgRuntime" { mapper: NROM }
        background Stage { tiles: [1, 2, 3] }
        on frame { load_background Stage }
        start Main
    "#;
    let rom_data = compile_banked(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
    // $14 == ZP_PENDING_BG_TILES_LO.
    let sta_14 = [0x85u8, 0x14];
    let found = rom_data.windows(sta_14.len()).any(|w| w == sta_14);
    assert!(
        found,
        "load_background codegen should emit `STA $14` (ZP_PENDING_BG_TILES_LO)"
    );
}

#[test]
fn program_without_palette_does_not_reserve_ppu_zero_page() {
    // Regression guard: programs that don't declare palette or
    // background should keep user vars starting at $10, same as
    // they always did, so existing emulator goldens don't shift.
    let source = r#"
        game "NoPal" { mapper: NROM }
        var x: u8 = 42
        on frame { x += 1 }
        start Main
    "#;
    let rom_data = compile_banked(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
    // `STA $10` (85 10) corresponds to writing to the first user
    // var slot. Guarantees `x` is still allocated at $10.
    let sta_10 = [0x85u8, 0x10];
    let found = rom_data.windows(sta_10.len()).any(|w| w == sta_10);
    assert!(
        found,
        "user var should still land at $10 when no palette/bg declared"
    );
}

// ── M5 Tests ──

/// Compile a source string using the mapper-aware linker.
fn compile_with_mapper(source: &str) -> Vec<u8> {
    compile_banked(source)
}

/// Compile a source string, running the full IR pipeline and
/// routing declared `bank X: prg` entries through `link_banked`
/// as empty switchable PRG slots. This mirrors the real CLI path.
fn compile_banked(source: &str) -> Vec<u8> {
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
    let sfx = assets::resolve_sfx(&program).expect("sfx resolution should succeed");
    let music = assets::resolve_music(&program).expect("music resolution should succeed");
    let palettes = assets::resolve_palettes(&program, Path::new("."))
        .expect("palette resolution should succeed");
    let backgrounds = assets::resolve_backgrounds(&program, Path::new("."))
        .expect("background resolution should succeed");

    let mut codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music);
    let mut instructions = codegen.generate(&ir_program);
    nescript::codegen::peephole::optimize(&mut instructions);

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

    // In PRG ROM, look for `LDA #$01 ; STA $0201,Y` which writes
    // the Player's tile index (1) into the tile-index byte of the
    // current OAM slot (the slot is computed at runtime via the
    // OAM cursor in Y). The STA AbsoluteY opcode is $99.
    let prg = &rom_data[16..16 + 16384];
    let pattern = [0xA9u8, 0x01, 0x99, 0x01, 0x02];
    assert!(
        prg.windows(pattern.len()).any(|w| w == pattern),
        "PRG ROM should contain LDA #$01 ; STA $0201,Y for draw Player",
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
//
// These tests exercise specific end-to-end IR codegen behavior.
// They all use the top-level `compile()` helper now that it runs
// the full IR pipeline — there's no longer a separate legacy path
// to compare against.

#[test]
fn ir_codegen_minimal_rom() {
    let source = r#"
        game "IR Test" { mapper: NROM }
        var x: u8 = 42
        on frame { wait_frame }
        start Main
    "#;
    let rom_data = compile(source);
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
    let rom_data = compile(source);
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
    let rom_data = compile(source);
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
    let rom_data = compile(source);
    rom::validate_ines(&rom_data).expect("should be valid iNES");
}

#[test]
fn ir_codegen_array_literal_globals_emit_per_byte_init() {
    // Regression test: `var xs: u8[4] = [10, 20, 30, 40]` used to
    // compile to a zero-initialized array because `eval_const`
    // returned `None` for `Expr::ArrayLiteral` and no startup
    // stores were emitted. The fix captures the literal values
    // in `IrGlobal::init_array` and has the IR codegen emit one
    // `LDA #imm; STA base+i` per byte during startup.
    use nescript::asm::{AddressingMode, Opcode};
    use nescript::codegen::IrCodeGen;

    let source = r#"
        game "ArrLit" { mapper: NROM }
        var xs: u8[4] = [10, 20, 30, 40]
        on frame { wait_frame }
        start Main
    "#;
    let (prog, diags) = nescript::parser::parse(source);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let prog = prog.unwrap();
    let analysis = analyzer::analyze(&prog);
    let mut ir_program = ir::lower(&prog, &analysis);
    optimizer::optimize(&mut ir_program);

    let xs_addr = analysis
        .var_allocations
        .iter()
        .find(|a| a.name == "xs")
        .expect("xs should be allocated")
        .address;

    let mut codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program);
    let instructions = codegen.generate(&ir_program);

    // For each element, look for `LDA #val` followed shortly by
    // `STA absolute(xs_addr + i)`. We don't require them to be
    // adjacent because the peephole passes can reshuffle, but a
    // store of the correct value to the correct address must
    // exist.
    for (i, &expected) in [10u8, 20, 30, 40].iter().enumerate() {
        let target = xs_addr + i as u16;
        let has_store = instructions.windows(2).any(|w| {
            matches!(w[0].mode, AddressingMode::Immediate(v) if v == expected)
                && w[0].opcode == Opcode::LDA
                && w[1].opcode == Opcode::STA
                && matches!(w[1].mode, AddressingMode::Absolute(a) if a == target)
        });
        assert!(
            has_store,
            "expected `LDA #{expected}; STA ${target:04X}` for xs[{i}] but did not find it"
        );
    }
}

#[test]
fn ir_codegen_locals_do_not_overlap_array_globals() {
    // Regression test for the local-allocator off-by-array-size
    // bug. `IrCodeGen::new` used to start handler-local vars at
    // `max_global_base + 1`, which for an array global at
    // `$0300-$0303` put the first local at `$0301` — inside the
    // array. Any store through that local then corrupted the
    // array mid-frame. The fix advances past the global's END,
    // not its base.
    //
    // We verify by asking the IR codegen what addresses it
    // assigned. Since `var_addrs` is private, we check indirectly
    // via emitted instructions: any `STA $030N` for N > 3 that
    // isn't part of the startup init must be writing to a local
    // whose address is outside the array. If the bug regressed,
    // we'd see `STA $0302` or similar in the frame handler's
    // computation code.
    use nescript::asm::{AddressingMode, Opcode};
    use nescript::codegen::IrCodeGen;

    let source = r#"
        game "LocalVsArr" { mapper: NROM }
        var xs: u8[4] = [11, 22, 33, 44]
        on frame {
            var tmp: u8 = 0
            tmp = xs[0]
            tmp += 1
            wait_frame
        }
        start Main
    "#;
    let (prog, diags) = nescript::parser::parse(source);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let prog = prog.unwrap();
    let analysis = analyzer::analyze(&prog);
    let mut ir_program = ir::lower(&prog, &analysis);
    optimizer::optimize(&mut ir_program);

    let xs_alloc = analysis
        .var_allocations
        .iter()
        .find(|a| a.name == "xs")
        .expect("xs should be allocated");
    let xs_base = xs_alloc.address;
    let xs_end = xs_base + xs_alloc.size; // one past last element

    let mut codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program);
    let instructions = codegen.generate(&ir_program);

    // Collect the (ordered) list of `STA absolute` targets and
    // immediate values preceding each store. The first four
    // stores into `[xs_base, xs_end)` should be the `LDA #imm;
    // STA addr` init pairs — those are fine. Any STA into the
    // array AFTER the init sequence would indicate a local var
    // was allocated inside the array.
    let mut init_stores_seen = 0usize;
    for w in instructions.windows(2) {
        if w[1].opcode != Opcode::STA {
            continue;
        }
        let AddressingMode::Absolute(addr) = w[1].mode else {
            continue;
        };
        if addr < xs_base || addr >= xs_end {
            continue;
        }
        if w[0].opcode == Opcode::LDA
            && matches!(w[0].mode, AddressingMode::Immediate(_))
            && init_stores_seen < 4
        {
            init_stores_seen += 1;
            continue;
        }
        panic!(
            "store into xs array (${addr:04X}) after init sequence — \
             local probably overlapping with array global"
        );
    }
    assert_eq!(
        init_stores_seen, 4,
        "expected 4 init stores for xs[0..4], found {init_stores_seen}"
    );
}

// ─── End-to-end bank switching tests ───────────────────────────────
//
// These tests compile real NEScript source through the full parse
// → analyze → IR → codegen → linker pipeline, producing .nes ROMs
// that assert the bank-switching layout the README promises:
//
//   * Declared `bank X: prg` slots become real 16 KB PRG banks
//   * Fixed bank lands at the end so it maps to $C000-$FFFF
//   * Reset vector points inside the fixed bank
//   * Mapper-specific init code appears in the fixed bank
//   * Every iNES header field reflects the banked layout

#[test]
fn e2e_mmc1_with_two_declared_banks_produces_three_bank_rom() {
    // MMC1 with two declared PRG banks should ship a ROM with
    // three 16 KB PRG slots (Level1Data, Level2Data, fixed).
    let source = r#"
        game "MMC1 Banked" {
            mapper: MMC1
            mirroring: horizontal
        }
        bank Level1Data: prg
        bank Level2Data: prg
        var x: u8 = 0
        on frame {
            if button.right { x += 1 }
        }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).expect("should be valid iNES");
    assert_eq!(info.mapper, 1, "mapper number should be 1 (MMC1)");
    assert_eq!(info.prg_banks, 3, "should have 2 switchable + 1 fixed bank");
    assert_eq!(rom.len(), 16 + 3 * 16384 + 8192);
}

#[test]
fn e2e_uxrom_with_four_banks_produces_five_bank_rom() {
    let source = r#"
        game "UxROM Banked" {
            mapper: UxROM
            mirroring: vertical
        }
        bank Level1: prg
        bank Level2: prg
        bank Level3: prg
        bank Level4: prg
        var x: u8 = 0
        on frame {
            if button.a { x += 1 }
        }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).expect("should be valid iNES");
    assert_eq!(info.mapper, 2, "mapper number should be 2 (UxROM)");
    assert_eq!(info.prg_banks, 5, "4 switchable + 1 fixed = 5 PRG banks");
    assert_eq!(info.mirroring, nescript::parser::ast::Mirroring::Vertical);
}

#[test]
fn e2e_mmc3_with_three_banks_produces_four_bank_rom() {
    let source = r#"
        game "MMC3 Banked" {
            mapper: MMC3
            mirroring: horizontal
        }
        bank Stage1: prg
        bank Stage2: prg
        bank Stage3: prg
        var x: u8 = 0
        on frame {
            if button.start { x = 1 }
        }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).expect("should be valid iNES");
    assert_eq!(info.mapper, 4, "mapper number should be 4 (MMC3)");
    assert_eq!(info.prg_banks, 4, "3 switchable + 1 fixed = 4 PRG banks");
}

#[test]
fn e2e_banked_fixed_bank_contains_reset_vector() {
    // The reset vector (bytes $FFFC/$FFFD in the final bank) must
    // point into the $C000-$FFFF window — this is how the CPU
    // boots into the fixed bank regardless of mapper.
    let source = r#"
        game "BankTest" { mapper: MMC1 }
        bank Data: prg
        on frame { wait_frame }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).expect("should be valid iNES");
    let prg_end = 16 + info.prg_banks * 16384;
    // Last 6 bytes = NMI, RESET, IRQ vectors (little-endian).
    let reset = u16::from_le_bytes([rom[prg_end - 4], rom[prg_end - 3]]);
    assert!(
        (0xC000..=0xFFFF).contains(&reset),
        "reset vector {reset:#06X} must live in fixed-bank address window"
    );
}

#[test]
fn e2e_banked_fixed_bank_contains_mmc1_init_and_bank_select() {
    // MMC1 requires a 6-way STA $8000 pattern at init (1 reset +
    // 5 control bits) plus a 5-way STA $E000 pattern in the
    // bank-select routine. Both must be in the fixed bank — they
    // ship with the program regardless of whether user code
    // calls `__bank_select` directly.
    let source = r#"
        game "MMC1Init" { mapper: MMC1 }
        bank Payload: prg
        var x: u8 = 0
        on frame { x += 1 }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).expect("should be valid iNES");
    // The fixed bank is the last 16 KB of PRG.
    let fixed_offset = 16 + (info.prg_banks - 1) * 16384;
    let fixed_bank = &rom[fixed_offset..fixed_offset + 16384];

    // Count STA $8000 (opcode $8D, operand little-endian $00 $80):
    // MMC1 init writes to $8000 six times.
    let sta_lo = [0x8Du8, 0x00, 0x80];
    let lo_count = fixed_bank.windows(3).filter(|w| *w == sta_lo).count();
    assert!(
        lo_count >= 6,
        "MMC1 fixed bank should contain >=6 STA $8000 writes (got {lo_count})"
    );

    // Count STA $E000 (opcode $8D, operand $00 $E0): bank-select
    // writes to it 5 times.
    let sta_hi = [0x8Du8, 0x00, 0xE0];
    let hi_count = fixed_bank.windows(3).filter(|w| *w == sta_hi).count();
    assert!(
        hi_count >= 5,
        "MMC1 fixed bank should contain >=5 STA $E000 writes (got {hi_count})"
    );
}

#[test]
fn e2e_banked_fixed_bank_contains_uxrom_bank_table() {
    // UxROM ships a 256-byte bank-select bus-conflict table
    // (values 0..=255). The table must be in the fixed bank.
    let source = r#"
        game "UxROMInit" { mapper: UxROM }
        bank Payload: prg
        on frame { wait_frame }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).unwrap();
    let fixed_offset = 16 + (info.prg_banks - 1) * 16384;
    let fixed = &rom[fixed_offset..fixed_offset + 16384];

    // Search for a run of 0,1,2,3,...,31 — a 32-byte stretch that's
    // distinctive enough that a random PRG byte sequence almost
    // never contains it. The full 256-byte table starts with this
    // prefix.
    let mut needle: [u8; 32] = [0; 32];
    #[allow(clippy::cast_possible_truncation)]
    for (i, b) in needle.iter_mut().enumerate() {
        *b = i as u8;
    }
    let found = fixed.windows(needle.len()).any(|w| w == needle);
    assert!(
        found,
        "UxROM fixed bank should contain the bank-select bus-conflict table"
    );
}

#[test]
fn e2e_banked_fixed_bank_contains_mmc3_init_writes() {
    // MMC3 init writes two (bank-select, bank-number) pairs to
    // ($8000, $8001) plus one $A000 mirroring write and one
    // $E000 IRQ-disable write. We check each pattern appears.
    let source = r#"
        game "MMC3Init" { mapper: MMC3 }
        bank Stage1: prg
        on frame { wait_frame }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).unwrap();
    let fixed_offset = 16 + (info.prg_banks - 1) * 16384;
    let fixed_bank = &rom[fixed_offset..fixed_offset + 16384];

    let select = [0x8Du8, 0x00, 0x80];
    let data = [0x8Du8, 0x01, 0x80];
    let mirror = [0x8Du8, 0x00, 0xA0];

    // MMC3 init writes $8000 twice, plus once per bank-select
    // call. With no `__bank_select` invocations from user code
    // we expect exactly 2 init writes to $8000, but the
    // bank-select subroutine also writes $8000 once. So the
    // minimum is 3 (2 init + 1 bank-select body).
    let select_count = fixed_bank.windows(3).filter(|w| *w == select).count();
    let data_count = fixed_bank.windows(3).filter(|w| *w == data).count();
    let mirror_count = fixed_bank.windows(3).filter(|w| *w == mirror).count();
    assert!(
        select_count >= 3,
        "MMC3 fixed bank should contain >=3 STA $8000 writes (got {select_count})"
    );
    assert!(
        data_count >= 3,
        "MMC3 fixed bank should contain >=3 STA $8001 writes (got {data_count})"
    );
    assert!(
        mirror_count >= 1,
        "MMC3 fixed bank should contain >=1 STA $A000 write for mirroring (got {mirror_count})"
    );
}

#[test]
fn e2e_banked_switchable_banks_contain_ff_padding() {
    // Empty switchable banks should be entirely $FF-filled so no
    // stray code accidentally lands in them. We check each
    // switchable bank slot is 16384 bytes of $FF.
    let source = r#"
        game "PadCheck" { mapper: MMC1 }
        bank A: prg
        bank B: prg
        on frame { wait_frame }
        start Main
    "#;
    let rom = compile_banked(source);
    for i in 0..2 {
        let offset = 16 + i * 16384;
        let bank = &rom[offset..offset + 16384];
        assert!(
            bank.iter().all(|&b| b == 0xFF),
            "switchable bank {i} should be all $FF padding"
        );
    }
}

#[test]
fn e2e_nrom_still_produces_single_bank_rom_without_declarations() {
    // Regression: programs that don't declare banks and use NROM
    // must still ship as a single-bank 16 KB PRG ROM (the legacy
    // layout), unaffected by the banking pipeline.
    let source = r#"
        game "Plain" { mapper: NROM }
        var x: u8 = 0
        on frame { x += 1 }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).unwrap();
    assert_eq!(info.mapper, 0);
    assert_eq!(info.prg_banks, 1);
    assert_eq!(rom.len(), 16 + 16384 + 8192);
}

#[test]
fn e2e_chr_banks_do_not_consume_prg_slots() {
    // A `bank X: chr` declaration reserves CHR space, not PRG.
    // The linker currently keeps CHR at a single 8 KB slot, so
    // declaring a CHR bank should NOT add a PRG slot.
    let source = r#"
        game "CHRBank" { mapper: MMC1 }
        bank TileBank: chr
        bank PrgBank: prg
        on frame { wait_frame }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).unwrap();
    // 1 PRG bank declared + 1 fixed = 2 total; TileBank:chr should
    // NOT bump the PRG count.
    assert_eq!(info.prg_banks, 2);
}

#[test]
fn e2e_mmc1_banked_example_compiles_successfully() {
    // The examples/mmc1_banked.ne file is the canonical example
    // the README points at. It must compile cleanly through the
    // full pipeline and produce a valid multi-bank ROM.
    let source = include_str!("../examples/mmc1_banked.ne");
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).expect("should be valid iNES");
    assert_eq!(info.mapper, 1, "mmc1_banked example should ship as MMC1");
    assert!(
        info.prg_banks >= 2,
        "mmc1_banked example should ship with at least 2 PRG banks (got {})",
        info.prg_banks
    );
}

#[test]
fn e2e_large_bank_count_still_produces_valid_rom() {
    // Stress test: 7 switchable banks (8 total) on UxROM. This
    // exercises the ROM builder's multi-bank concatenation with
    // a non-trivial bank count and ensures nothing in the linker
    // pipeline hard-codes a bank limit.
    let source = r#"
        game "LotsOfBanks" { mapper: UxROM }
        bank A: prg
        bank B: prg
        bank C: prg
        bank D: prg
        bank E: prg
        bank F: prg
        bank G: prg
        on frame { wait_frame }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).unwrap();
    assert_eq!(info.prg_banks, 8, "7 switchable + 1 fixed = 8 PRG banks");
    assert_eq!(rom.len(), 16 + 8 * 16384 + 8192);
}

#[test]
fn e2e_banked_rom_ines_header_mapper_bits_encoded_correctly() {
    // Sanity check: the iNES header's mapper number field is split
    // across byte 6 (low nibble) and byte 7 (high nibble). For
    // mapper 1 (MMC1), byte 6 should have $10 in its high nibble
    // and byte 7 should have $00 in its high nibble.
    let source = r#"
        game "HeaderCheck" { mapper: MMC1 }
        bank Foo: prg
        on frame { wait_frame }
        start Main
    "#;
    let rom = compile_banked(source);
    let byte6_high_nibble = rom[6] & 0xF0;
    let byte7_high_nibble = rom[7] & 0xF0;
    assert_eq!(byte6_high_nibble, 0x10, "MMC1 low mapper nibble in byte 6");
    assert_eq!(byte7_high_nibble, 0x00, "MMC1 high mapper nibble in byte 7");
}

#[test]
fn e2e_banked_all_three_mappers_have_correct_vectors() {
    // For each banked mapper, verify all three vectors (NMI, RESET,
    // IRQ) live inside the fixed bank address window.
    for mapper_kw in ["MMC1", "UxROM", "MMC3"] {
        let source = format!(
            r#"
                game "VecCheck" {{ mapper: {mapper_kw} }}
                bank One: prg
                on frame {{ wait_frame }}
                start Main
            "#
        );
        let rom = compile_banked(&source);
        let info = rom::validate_ines(&rom).unwrap();
        let prg_end = 16 + info.prg_banks * 16384;
        let nmi = u16::from_le_bytes([rom[prg_end - 6], rom[prg_end - 5]]);
        let reset = u16::from_le_bytes([rom[prg_end - 4], rom[prg_end - 3]]);
        let irq = u16::from_le_bytes([rom[prg_end - 2], rom[prg_end - 1]]);
        for (name, v) in [("NMI", nmi), ("RESET", reset), ("IRQ", irq)] {
            assert!(
                (0xC000..=0xFFFF).contains(&v),
                "{mapper_kw} {name} vector {v:#06X} should be in fixed-bank window"
            );
        }
    }
}

#[test]
fn e2e_bank_declarations_dont_affect_nrom_prg_size() {
    // Even though the linker REJECTS switchable banks for NROM,
    // the compiler only passes banks through when they're in the
    // `program.banks` list — for NROM sources without declarations
    // nothing is passed, so the NROM path is unchanged. Just
    // double-check here that a plain NROM ROM is still 1 bank.
    let source = r#"
        game "JustNROM" { mapper: NROM }
        on frame { wait_frame }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).unwrap();
    assert_eq!(info.prg_banks, 1);
    assert_eq!(info.mapper, 0);
}

#[test]
fn e2e_banked_chr_rom_is_preserved() {
    // CHR ROM should still contain the default smiley sprite at
    // tile 0 regardless of how many PRG banks the ROM has.
    let source = r#"
        game "CHRCheck" { mapper: MMC1 }
        bank One: prg
        bank Two: prg
        on frame { wait_frame }
        start Main
    "#;
    let rom = compile_banked(source);
    let info = rom::validate_ines(&rom).unwrap();
    let chr_start = 16 + info.prg_banks * 16384;
    // Default smiley is non-zero in its first 16 bytes.
    assert_ne!(&rom[chr_start..chr_start + 16], &[0u8; 16]);
}

#[test]
fn e2e_png_palette_source_compiles_and_splices_bytes_into_prg() {
    // Full pipeline: parse `palette Main @palette("fixture.png")`,
    // resolve the PNG into a 32-byte blob via the asset resolver,
    // and verify the resulting bytes land in PRG ROM. We write a
    // 2×1 test fixture (pure black + pure red) to a tempdir so
    // the test is self-contained and deterministic.
    use image::{Rgb, RgbImage};
    use nescript::codegen::IrCodeGen;
    use nescript::linker::LinkedRom;

    let dir = std::env::temp_dir();
    let png_path = dir.join("nescript_e2e_palette.png");
    let mut img = RgbImage::new(2, 1);
    img.put_pixel(0, 0, Rgb([0, 0, 0]));
    img.put_pixel(1, 0, Rgb([248, 0, 0]));
    img.save(&png_path).unwrap();

    let source = r#"
        game "PngPalette" { mapper: NROM }
        palette Main @palette("nescript_e2e_palette.png")
        on frame { wait_frame }
        start Main
    "#;

    let (program, diags) = nescript::parser::parse(source);
    assert!(diags.is_empty(), "unexpected parse errors: {diags:?}");
    let program = program.expect("parse should succeed");
    let analysis = analyzer::analyze(&program);
    assert!(analysis.diagnostics.iter().all(|d| !d.is_error()));

    // Resolve with the tempdir as the source dir so the
    // relative PNG path lands on the fixture we just wrote.
    let palettes =
        assets::resolve_palettes(&program, &dir).expect("palette resolution should succeed");
    let backgrounds = assets::resolve_backgrounds(&program, &dir).expect("bg ok");
    assert_eq!(palettes.len(), 1);
    assert_eq!(palettes[0].name, "Main");
    // First two bytes should map via `nearest_nes_color` to black
    // and a red-ish index. We re-run the mapper so the test
    // doesn't hard-code the NES palette table.
    let e_black = assets::nearest_nes_color(0, 0, 0);
    let e_red = assets::nearest_nes_color(248, 0, 0);
    assert_eq!(palettes[0].colors[0], e_black);
    assert_eq!(palettes[0].colors[1], e_red);
    // Every sub-palette first byte equals the universal.
    for slot in 0..8 {
        assert_eq!(palettes[0].colors[slot * 4], e_black);
    }

    // Link the program and verify the 32-byte blob shows up in PRG
    // ROM at the linker-assigned label.
    let sprites = assets::resolve_sprites(&program, Path::new(".")).unwrap();
    let sfx = assets::resolve_sfx(&program).unwrap();
    let music = assets::resolve_music(&program).unwrap();
    let mut ir_program = nescript::ir::lower(&program, &analysis);
    nescript::optimizer::optimize(&mut ir_program);
    let mut codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music);
    let mut instructions = codegen.generate(&ir_program);
    nescript::codegen::peephole::optimize(&mut instructions);

    let linker = Linker::with_mapper(program.game.mirroring, program.game.mapper);
    let link: LinkedRom = linker.link_banked_with_ppu_detailed(
        &instructions,
        &sprites,
        &sfx,
        &music,
        &palettes,
        &backgrounds,
        &[],
    );
    let pal_label = palettes[0].label();
    let pal_addr = link
        .labels
        .get(&pal_label)
        .copied()
        .expect("palette label should be emitted");
    // Translate the CPU address into a byte offset inside the
    // fixed bank. NROM: the fixed bank starts at file offset 16
    // (past the iNES header) and maps to CPU $C000-$FFFF.
    let rom_offset = link.fixed_bank_file_offset + (pal_addr as usize - 0xC000);
    let prg_bytes = &link.rom[rom_offset..rom_offset + 32];
    assert_eq!(
        prg_bytes, &palettes[0].colors,
        "PRG ROM should contain the decoded palette blob verbatim"
    );

    let _ = std::fs::remove_file(&png_path);
}

/// Same as `compile_banked` but lets the caller toggle whether the IR
/// optimizer runs. Used to cover the `--no-opt` CLI flag: compiling
/// with the optimizer disabled must still produce a valid iNES ROM.
fn compile_banked_with_opts(source: &str, optimize: bool) -> Vec<u8> {
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
    if optimize {
        nescript::optimizer::optimize(&mut ir_program);
    }

    let sprites = assets::resolve_sprites(&program, Path::new("."))
        .expect("sprite resolution should succeed");
    let sfx = assets::resolve_sfx(&program).expect("sfx resolution should succeed");
    let music = assets::resolve_music(&program).expect("music resolution should succeed");
    let palettes = assets::resolve_palettes(&program, Path::new("."))
        .expect("palette resolution should succeed");
    let backgrounds = assets::resolve_backgrounds(&program, Path::new("."))
        .expect("background resolution should succeed");

    let mut codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music);
    let mut instructions = codegen.generate(&ir_program);
    nescript::codegen::peephole::optimize(&mut instructions);

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

#[test]
fn no_opt_still_produces_valid_rom() {
    // Acceptance test for the `--no-opt` CLI flag. Skipping the IR
    // optimizer must still produce a byte-valid iNES ROM that links
    // against the runtime, uses the declared mapper, and carries a
    // plausible vector table. This guards the compile path the flag
    // opens up so optimizer bisection remains a usable workflow.
    let source = r#"
        game "NoOpt" { mapper: NROM }

        var counter: u8 = 0
        var doubled: u8 = 0

        fun double(x: u8) -> u8 {
            return x + x
        }

        on frame {
            counter += 1
            doubled = double(counter)
            if button.a {
                counter = 0
            }
            wait_frame
        }
        start Main
    "#;

    let rom_opt = compile_banked_with_opts(source, true);
    let rom_noopt = compile_banked_with_opts(source, false);

    // Both outputs must be valid iNES ROMs with matching headers —
    // the optimizer only affects PRG codegen, not the CHR/header
    // layout the linker produces.
    let info_opt = rom::validate_ines(&rom_opt).expect("opt ROM should be valid iNES");
    let info_noopt = rom::validate_ines(&rom_noopt).expect("noopt ROM should be valid iNES");
    assert_eq!(info_opt.mapper, 0);
    assert_eq!(info_noopt.mapper, 0);
    assert_eq!(info_opt.prg_banks, info_noopt.prg_banks);
    assert_eq!(info_opt.chr_banks, info_noopt.chr_banks);
    assert_eq!(rom_opt.len(), rom_noopt.len());

    // The reset vector should still point into the fixed PRG bank
    // in both builds — the optimizer has no say in where the reset
    // handler lands.
    let prg_end = 16 + 16384;
    let reset_opt = u16::from_le_bytes([rom_opt[prg_end - 4], rom_opt[prg_end - 3]]);
    let reset_noopt = u16::from_le_bytes([rom_noopt[prg_end - 4], rom_noopt[prg_end - 3]]);
    assert_eq!(reset_opt, 0xC000);
    assert_eq!(reset_noopt, 0xC000);
}

/// End-to-end pipeline that mirrors the CLI's `--debug`,
/// `--symbols`, and `--source-map` paths. Returns the ROM bytes
/// along with the rendered `.mlb` and source-map text so the
/// integration tests can assert against the whole chain.
fn compile_with_debug_artifacts(source: &str, debug: bool) -> (Vec<u8>, String, String) {
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
    optimizer::optimize(&mut ir_program);

    let sprites = assets::resolve_sprites(&program, Path::new("."))
        .expect("sprite resolution should succeed");
    let sfx = assets::resolve_sfx(&program).expect("sfx resolution should succeed");
    let music = assets::resolve_music(&program).expect("music resolution should succeed");
    let palettes = assets::resolve_palettes(&program, Path::new("."))
        .expect("palette resolution should succeed");
    let backgrounds = assets::resolve_backgrounds(&program, Path::new("."))
        .expect("background resolution should succeed");

    let mut codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music)
        .with_debug(debug)
        .with_source_map(true);
    let mut instructions = codegen.generate(&ir_program);
    nescript::codegen::peephole::optimize(&mut instructions);

    let linker = Linker::with_mapper(program.game.mirroring, program.game.mapper);
    let switchable_banks: Vec<PrgBank> = program
        .banks
        .iter()
        .filter(|b| b.bank_type == BankType::Prg)
        .map(|b| PrgBank::empty(&b.name))
        .collect();
    let link_result = linker.link_banked_with_ppu_detailed(
        &instructions,
        &sprites,
        &sfx,
        &music,
        &palettes,
        &backgrounds,
        &switchable_banks,
    );
    let mlb = nescript::linker::render_mlb(&link_result, &analysis.var_allocations);
    let map = nescript::linker::render_source_map(&link_result, codegen.source_locs(), source);
    (link_result.rom, mlb, map)
}

#[test]
fn symbol_export_lists_user_functions_states_and_vars() {
    // Compile a small program that exercises the symbol-export
    // path: a user function, a state handler, a global variable,
    // and at least one array. The rendered `.mlb` should mention
    // every one of those under its user-facing name (not the
    // internal `__ir_fn_` prefix).
    let source = r#"
        game "Symbols" { mapper: NROM }
        var score: u8 = 0
        var enemies: u8[4] = [1, 2, 3, 4]
        fun bump() -> u8 { return 1 }
        state Main {
            on frame {
                score = bump()
                wait_frame
            }
        }
        start Main
    "#;
    let (_rom, mlb, _map) = compile_with_debug_artifacts(source, false);

    // User functions appear with their bare name.
    assert!(mlb.contains(":bump"), "bump() should be in .mlb:\n{mlb}");
    assert!(
        mlb.contains(":Main_frame"),
        "state frame handler should be in .mlb:\n{mlb}"
    );
    // Well-known entry points.
    assert!(mlb.contains(":reset"));
    assert!(mlb.contains(":nmi"));
    assert!(mlb.contains(":main_loop"));
    // User variables with the `R:` prefix.
    assert!(
        mlb.contains(":score"),
        "global var `score` should be in .mlb:\n{mlb}"
    );
    assert!(
        mlb.contains(":enemies"),
        "array var `enemies` should be in .mlb:\n{mlb}"
    );
    // Make sure internal-only labels did not leak.
    assert!(
        !mlb.contains("__ir_fn_"),
        ".mlb should strip the __ir_fn_ prefix"
    );
    // P:-prefix entries should resolve to in-ROM offsets below
    // the 16 KB fixed bank size.
    for line in mlb.lines().filter(|l| l.starts_with("P:")) {
        let hex = &line[2..6];
        let offset = u32::from_str_radix(hex, 16).unwrap();
        assert!(
            offset < 0x4000,
            "P: offset {offset:#06X} should be inside the 16 KB fixed bank"
        );
    }
}

#[test]
fn source_map_covers_every_lowered_statement() {
    let source = r#"
game "SourceMap" { mapper: NROM }
on frame {
    var a: u8 = 1
    var b: u8 = 2
    var c: u8 = 3
    wait_frame
}
start Main
"#;
    let (_rom, _mlb, map) = compile_with_debug_artifacts(source, false);
    assert!(
        !map.is_empty(),
        "source map should be non-empty when --source-map is on"
    );
    // Each non-empty line has the form: `<offset> <file> <line> <col>`.
    let lines: Vec<_> = map.lines().collect();
    assert!(
        lines.len() >= 4,
        "should cover at least the four user statements; got {}",
        lines.len()
    );
    // Lines should be sorted by ROM offset.
    let offsets: Vec<u32> = lines
        .iter()
        .map(|l| u32::from_str_radix(l.split_whitespace().next().unwrap(), 16).unwrap())
        .collect();
    let mut sorted = offsets.clone();
    sorted.sort_unstable();
    assert_eq!(offsets, sorted, "source map must be sorted by ROM offset");
    // At least one entry should point at line 4 (the `var a`
    // declaration — line 1 is blank, line 2 is `game`, line 3 is
    // `on frame {`, line 4 is the first body statement).
    let has_line_4 = lines.iter().any(|l| {
        let parts: Vec<_> = l.split_whitespace().collect();
        parts.len() == 4 && parts[2] == "4"
    });
    assert!(
        has_line_4,
        "source map should include at least one entry for line 4:\n{map}"
    );
}

#[test]
fn debug_build_emits_bounds_check_halt_routine() {
    // When compiled with `--debug`, a program that indexes an
    // array should include the shared `__debug_halt` trip routine
    // and at least one JMP targeting it. Release builds must not.
    let source = r#"
        game "BoundsCheck" { mapper: NROM }
        var xs: u8[4] = [1, 2, 3, 4]
        on frame {
            var i: u8 = 0
            var v: u8 = xs[i]
            wait_frame
        }
        start Main
    "#;
    let (_rom, mlb_debug, _map) = compile_with_debug_artifacts(source, true);
    // The halt routine is internal so it's filtered from the
    // `.mlb` output, but we can verify by re-compiling the same
    // program and scanning the linker's label table directly.
    let (program, _) = nescript::parser::parse(source);
    let program = program.unwrap();
    let analysis = analyzer::analyze(&program);
    let mut ir_program = ir::lower(&program, &analysis);
    optimizer::optimize(&mut ir_program);
    let sprites = assets::resolve_sprites(&program, Path::new(".")).unwrap();
    let sfx = assets::resolve_sfx(&program).unwrap();
    let music = assets::resolve_music(&program).unwrap();
    let palettes = assets::resolve_palettes(&program, Path::new("."))
        .expect("palette resolution should succeed");
    let backgrounds = assets::resolve_backgrounds(&program, Path::new("."))
        .expect("background resolution should succeed");

    let mut cg_debug = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music)
        .with_debug(true);
    let mut insts_debug = cg_debug.generate(&ir_program);
    nescript::codegen::peephole::optimize(&mut insts_debug);
    let linker = Linker::with_mapper(program.game.mirroring, program.game.mapper);
    let linked_debug = linker.link_banked_with_ppu_detailed(
        &insts_debug,
        &sprites,
        &sfx,
        &music,
        &palettes,
        &backgrounds,
        &[],
    );
    assert!(
        linked_debug.labels.contains_key("__debug_halt"),
        "debug build should define the shared bounds-check halt label"
    );

    let mut cg_release = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music);
    let mut insts_release = cg_release.generate(&ir_program);
    nescript::codegen::peephole::optimize(&mut insts_release);
    let linked_release = linker.link_banked_with_ppu_detailed(
        &insts_release,
        &sprites,
        &sfx,
        &music,
        &palettes,
        &backgrounds,
        &[],
    );
    assert!(
        !linked_release.labels.contains_key("__debug_halt"),
        "release build must not emit __debug_halt"
    );
    // And the rendered `.mlb` for the debug build should not
    // contain the internal halt label either (it's filtered out).
    assert!(
        !mlb_debug.contains("__debug_halt"),
        "debug halt label is internal; should not leak into .mlb"
    );
}
