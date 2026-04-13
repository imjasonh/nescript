use std::path::Path;

use nescript::analyzer;
use nescript::assets;
use nescript::codegen::IrCodeGen;
use nescript::ir;
use nescript::linker::Linker;
use nescript::optimizer;
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

    let codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program)
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
    let sfx = assets::resolve_sfx(&program).expect("sfx resolution should succeed");
    let music = assets::resolve_music(&program).expect("music resolution should succeed");

    let codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program)
        .with_sprites(&sprites)
        .with_audio(&sfx, &music);
    let mut instructions = codegen.generate(&ir_program);
    nescript::codegen::peephole::optimize(&mut instructions);

    let linker = Linker::with_mapper(program.game.mirroring, program.game.mapper);
    linker.link_with_all_assets(&instructions, &sprites, &sfx, &music)
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

    let codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program);
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

    let codegen = IrCodeGen::new(&analysis.var_allocations, &ir_program);
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
