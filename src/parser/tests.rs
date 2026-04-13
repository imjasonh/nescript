use super::*;
use crate::parser::ast::*;

fn parse_ok(input: &str) -> Program {
    let (prog, diags) = parse(input);
    assert!(
        diags.is_empty(),
        "unexpected errors: {diags:?}\ninput: {input}"
    );
    prog.expect("expected successful parse")
}

fn parse_err(input: &str) -> Vec<crate::errors::ErrorCode> {
    let (_, diags) = parse(input);
    assert!(!diags.is_empty(), "expected errors but got none");
    diags.into_iter().map(|d| d.code).collect()
}

const MINIMAL_GAME: &str = r#"
game "Test" { mapper: NROM }
var px: u8 = 0
on frame {
    px = 1
}
start Main
"#;

// ── Game declaration ──

#[test]
fn parse_game_decl() {
    let prog = parse_ok(MINIMAL_GAME);
    assert_eq!(prog.game.name, "Test");
    assert_eq!(prog.game.mapper, Mapper::NROM);
}

#[test]
fn parse_game_with_mirroring() {
    let src = r#"
        game "Test" { mapper: NROM mirroring: vertical }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.game.mirroring, Mirroring::Vertical);
}

// ── Variables ──

#[test]
fn parse_global_var() {
    let prog = parse_ok(MINIMAL_GAME);
    assert_eq!(prog.globals.len(), 1);
    assert_eq!(prog.globals[0].name, "px");
    assert_eq!(prog.globals[0].var_type, NesType::U8);
}

#[test]
fn parse_multiple_globals() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 128
        var py: u8 = 120
        on frame { px = 1 }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.globals.len(), 2);
    assert_eq!(prog.globals[0].name, "px");
    assert_eq!(prog.globals[1].name, "py");
}

// ── Constants ──

#[test]
fn parse_const_decl() {
    let src = r#"
        game "Test" { mapper: NROM }
        const SPEED: u8 = 2
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.constants.len(), 1);
    assert_eq!(prog.constants[0].name, "SPEED");
}

// ── On frame ──

#[test]
fn parse_on_frame_implicit_state() {
    let prog = parse_ok(MINIMAL_GAME);
    assert_eq!(prog.states.len(), 1);
    assert_eq!(prog.states[0].name, "Main");
    assert!(prog.states[0].on_frame.is_some());
    assert_eq!(prog.start_state, "Main");
}

// ── Statements ──

#[test]
fn parse_if_statement() {
    let src = r#"
        game "Test" { mapper: NROM }
        on frame {
            if button.right { px += 2 }
        }
        var px: u8 = 0
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 1);
    assert!(matches!(frame.statements[0], Statement::If(..)));
}

#[test]
fn parse_if_else() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x == 0 {
                x = 1
            } else {
                x = 0
            }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(_, _, _, else_block, _) => {
            assert!(else_block.is_some());
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn parse_if_else_if() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x == 0 {
                x = 1
            } else if x == 1 {
                x = 2
            } else {
                x = 0
            }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(_, _, else_ifs, else_block, _) => {
            assert_eq!(else_ifs.len(), 1);
            assert!(else_block.is_some());
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn parse_while_loop() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            while x < 10 { x += 1 }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert!(matches!(frame.statements[0], Statement::While(..)));
}

#[test]
fn parse_draw_statement() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        var py: u8 = 0
        on frame {
            draw Smiley at: (px, py)
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::Draw(d) => {
            assert_eq!(d.sprite_name, "Smiley");
        }
        other => panic!("expected Draw, got {other:?}"),
    }
}

#[test]
fn parse_draw_with_frame() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        var py: u8 = 0
        on frame {
            draw Smiley at: (px, py) frame: 0
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::Draw(d) => {
            assert!(d.frame.is_some());
        }
        other => panic!("expected Draw, got {other:?}"),
    }
}

// ── Expressions ──

#[test]
fn parse_arithmetic() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            x = 1 + 2 * 3
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    // x = 1 + (2 * 3) -- multiplication binds tighter
    match &frame.statements[0] {
        Statement::Assign(_, _, Expr::BinaryOp(_, BinOp::Add, right, _), _) => {
            assert!(matches!(
                right.as_ref(),
                Expr::BinaryOp(_, BinOp::Mul, _, _)
            ));
        }
        other => panic!("expected assignment with Add, got {other:?}"),
    }
}

#[test]
fn parse_button_read() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        on frame {
            if button.right { px += 1 }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(Expr::ButtonRead(_, button, _), _, _, _, _) => {
            assert_eq!(button, "right");
        }
        other => panic!("expected if with button read, got {other:?}"),
    }
}

#[test]
fn parse_comparison_ops() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x >= 10 { x = 0 }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(Expr::BinaryOp(_, BinOp::GtEq, _, _), _, _, _, _) => {}
        other => panic!("expected if with GtEq, got {other:?}"),
    }
}

// ── State declarations ──

#[test]
fn parse_explicit_state() {
    let src = r#"
        game "Test" { mapper: NROM }
        state Title {
            on enter { wait_frame }
            on frame { wait_frame }
        }
        start Title
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.states.len(), 1);
    assert_eq!(prog.states[0].name, "Title");
    assert!(prog.states[0].on_enter.is_some());
    assert!(prog.states[0].on_frame.is_some());
}

// ── Error cases ──

#[test]
fn parse_missing_game() {
    use crate::errors::ErrorCode;
    let codes = parse_err("var x: u8 = 0\nstart Main");
    assert!(codes.contains(&ErrorCode::E0504));
}

#[test]
fn parse_missing_start_with_explicit_state() {
    use crate::errors::ErrorCode;
    // Explicit state without `start` declaration should fail
    let codes = parse_err(
        r#"
        game "T" { mapper: NROM }
        state Foo { on frame { wait_frame } }
    "#,
    );
    assert!(codes.contains(&ErrorCode::E0504));
}

#[test]
fn parse_on_frame_implies_start() {
    // Top-level `on frame` auto-creates implicit "Main" state and start
    let prog = parse_ok(r#"game "T" { mapper: NROM } on frame { wait_frame }"#);
    assert_eq!(prog.start_state, "Main");
}

// ── Full M1 program ──

#[test]
fn parse_full_m1_program() {
    let src = r#"
        game "Hello Sprite" {
            mapper: NROM
        }

        var px: u8 = 128
        var py: u8 = 120

        on frame {
            if button.right { px += 2 }
            if button.left  { px -= 2 }
            if button.down  { py += 2 }
            if button.up    { py -= 2 }

            draw Smiley at: (px, py)
        }

        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.game.name, "Hello Sprite");
    assert_eq!(prog.globals.len(), 2);
    assert_eq!(prog.states.len(), 1);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 5); // 4 ifs + 1 draw
}

// ── Milestone 2: Functions ──

#[test]
fn parse_function_decl() {
    let src = r#"
        game "Test" { mapper: NROM }
        fun add(a: u8, b: u8) -> u8 {
            return a + b
        }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.functions.len(), 1);
    let f = &prog.functions[0];
    assert_eq!(f.name, "add");
    assert!(!f.is_inline);
    assert_eq!(f.params.len(), 2);
    assert_eq!(f.params[0].name, "a");
    assert_eq!(f.params[0].param_type, NesType::U8);
    assert_eq!(f.params[1].name, "b");
    assert_eq!(f.params[1].param_type, NesType::U8);
    assert_eq!(f.return_type, Some(NesType::U8));
    assert_eq!(f.body.statements.len(), 1);
}

#[test]
fn parse_inline_function() {
    let src = r#"
        game "Test" { mapper: NROM }
        inline fun double(x: u8) -> u8 {
            return x + x
        }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.functions.len(), 1);
    let f = &prog.functions[0];
    assert_eq!(f.name, "double");
    assert!(f.is_inline);
    assert_eq!(f.params.len(), 1);
    assert_eq!(f.params[0].name, "x");
    assert_eq!(f.return_type, Some(NesType::U8));
}

#[test]
fn parse_array_type() {
    let src = r#"
        game "Test" { mapper: NROM }
        var buf: u8[16] = [0, 0, 0]
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.globals.len(), 1);
    assert_eq!(
        prog.globals[0].var_type,
        NesType::Array(Box::new(NesType::U8), 16)
    );
    assert!(prog.globals[0].init.is_some());
}

#[test]
fn parse_array_literal() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            x = [1, 2, 3]
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::Assign(_, _, Expr::ArrayLiteral(elems, _), _) => {
            assert_eq!(elems.len(), 3);
        }
        other => panic!("expected assignment with ArrayLiteral, got {other:?}"),
    }
}

#[test]
fn parse_fast_var() {
    let src = r#"
        game "Test" { mapper: NROM }
        fast var x: u8 = 0
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.globals.len(), 1);
    assert_eq!(prog.globals[0].name, "x");
    assert_eq!(prog.globals[0].placement, Placement::Fast);
}

#[test]
fn parse_function_call_expr() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            x = add(1, 2)
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::Assign(_, _, Expr::Call(name, args, _), _) => {
            assert_eq!(name, "add");
            assert_eq!(args.len(), 2);
        }
        other => panic!("expected assignment with Call, got {other:?}"),
    }
}

// ── Milestone 3: Sprites / Palettes / Backgrounds ──

#[test]
fn parse_sprite_decl() {
    let src = r#"
        game "Test" { mapper: NROM }
        sprite Player {
            chr: [0x3C, 0x42, 0x81, 0x81, 0x81, 0x81, 0x42, 0x3C,
                  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.sprites.len(), 1);
    assert_eq!(prog.sprites[0].name, "Player");
    match &prog.sprites[0].chr_source {
        AssetSource::Inline(data) => {
            assert_eq!(data.len(), 16);
            assert_eq!(data[0], 0x3C);
        }
        other => panic!("expected Inline, got {other:?}"),
    }
}

// ── Audio subsystem: sfx / music declarations ──

#[test]
fn parse_sfx_decl_minimal() {
    let src = r#"
        game "T" { mapper: NROM }
        sfx Pickup {
            pitch: [0x60, 0x58, 0x50, 0x48]
            volume: [15, 12, 8, 4]
        }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.sfx.len(), 1);
    let sfx = &prog.sfx[0];
    assert_eq!(sfx.name, "Pickup");
    assert_eq!(sfx.pitch, vec![0x60, 0x58, 0x50, 0x48]);
    assert_eq!(sfx.volume, vec![15, 12, 8, 4]);
    // Default duty is 2 (50% square).
    assert_eq!(sfx.duty, 2);
}

#[test]
fn parse_sfx_decl_with_duty() {
    let src = r#"
        game "T" { mapper: NROM }
        sfx Zap {
            duty: 3
            pitch: [0x20, 0x22, 0x24]
            volume: [15, 10, 5]
        }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.sfx[0].duty, 3);
}

#[test]
fn parse_sfx_decl_rejects_mismatched_array_lengths() {
    let src = r#"
        game "T" { mapper: NROM }
        sfx Bad {
            pitch: [0x20, 0x22]
            volume: [15, 10, 5]
        }
        on frame { wait_frame }
        start Main
    "#;
    let (_, diags) = parse(src);
    assert!(
        diags.iter().any(crate::errors::Diagnostic::is_error),
        "mismatched pitch/volume lengths should error"
    );
}

#[test]
fn parse_sfx_decl_rejects_volume_over_15() {
    let src = r#"
        game "T" { mapper: NROM }
        sfx Bad {
            pitch: [0x20]
            volume: [16]
        }
        on frame { wait_frame }
        start Main
    "#;
    let (_, diags) = parse(src);
    assert!(
        diags.iter().any(crate::errors::Diagnostic::is_error),
        "volume > 15 should error"
    );
}

#[test]
fn parse_sfx_decl_rejects_duty_over_3() {
    let src = r#"
        game "T" { mapper: NROM }
        sfx Bad {
            duty: 4
            pitch: [0x20]
            volume: [8]
        }
        on frame { wait_frame }
        start Main
    "#;
    let (_, diags) = parse(src);
    assert!(
        diags.iter().any(crate::errors::Diagnostic::is_error),
        "duty > 3 should error"
    );
}

#[test]
fn parse_music_decl_minimal() {
    let src = r#"
        game "T" { mapper: NROM }
        music Theme {
            notes: [37, 8, 41, 8, 44, 8, 49, 16]
        }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.music.len(), 1);
    let m = &prog.music[0];
    assert_eq!(m.name, "Theme");
    assert_eq!(m.notes.len(), 4);
    assert_eq!(m.notes[0].pitch, 37);
    assert_eq!(m.notes[0].duration, 8);
    assert_eq!(m.notes[3].pitch, 49);
    assert_eq!(m.notes[3].duration, 16);
    // Default music loops.
    assert!(m.loops);
}

#[test]
fn parse_music_decl_with_full_properties() {
    let src = r#"
        game "T" { mapper: NROM }
        music Victory {
            duty: 0
            volume: 12
            repeat: false
            notes: [37, 10, 41, 10, 44, 10, 49, 20]
        }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    let m = &prog.music[0];
    assert_eq!(m.duty, 0);
    assert_eq!(m.volume, 12);
    assert!(!m.loops, "repeat: false should disable looping");
}

#[test]
fn parse_music_decl_rejects_odd_note_count() {
    let src = r#"
        game "T" { mapper: NROM }
        music Bad {
            notes: [37, 8, 41]
        }
        on frame { wait_frame }
        start Main
    "#;
    let (_, diags) = parse(src);
    assert!(
        diags.iter().any(crate::errors::Diagnostic::is_error),
        "odd note count should error — notes must come in (pitch, duration) pairs"
    );
}

#[test]
fn parse_music_decl_rejects_pitch_above_60() {
    let src = r#"
        game "T" { mapper: NROM }
        music Bad {
            notes: [100, 8]
        }
        on frame { wait_frame }
        start Main
    "#;
    let (_, diags) = parse(src);
    assert!(
        diags.iter().any(crate::errors::Diagnostic::is_error),
        "pitch above 60 should error"
    );
}

#[test]
fn parse_music_decl_rejects_zero_duration() {
    let src = r#"
        game "T" { mapper: NROM }
        music Bad {
            notes: [37, 0]
        }
        on frame { wait_frame }
        start Main
    "#;
    let (_, diags) = parse(src);
    assert!(
        diags.iter().any(crate::errors::Diagnostic::is_error),
        "zero duration should error"
    );
}

#[test]
fn parse_play_and_start_music_statements() {
    let src = r#"
        game "T" { mapper: NROM }
        on frame {
            play coin
            start_music theme
            stop_music
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let stmts = &prog.states[0].on_frame.as_ref().unwrap().statements;
    assert!(matches!(&stmts[0], Statement::Play(n, _) if n == "coin"));
    assert!(matches!(&stmts[1], Statement::StartMusic(n, _) if n == "theme"));
    assert!(matches!(&stmts[2], Statement::StopMusic(_)));
}

// ── Milestone 4: Optimization & Polish ──

#[test]
fn parse_cast_expression() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        var y: u16 = 0
        on frame {
            y = x as u16
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::Assign(_, _, Expr::Cast(inner, target_type, _), _) => {
            assert!(matches!(inner.as_ref(), Expr::Ident(name, _) if name == "x"));
            assert_eq!(*target_type, NesType::U16);
        }
        other => panic!("expected assignment with Cast, got {other:?}"),
    }
}

#[test]
fn parse_scroll_statement() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        var py: u8 = 0
        on frame {
            scroll(px, py)
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 1);
    match &frame.statements[0] {
        Statement::Scroll(x, y, _) => {
            assert!(matches!(x, Expr::Ident(name, _) if name == "px"));
            assert!(matches!(y, Expr::Ident(name, _) if name == "py"));
        }
        other => panic!("expected Scroll, got {other:?}"),
    }
}

// ── Milestone 5: Bank Switching & Release ──

#[test]
fn parse_mmc1_mapper() {
    let src = r#"
        game "Test" { mapper: MMC1 }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.game.mapper, Mapper::MMC1);
}

#[test]
fn parse_uxrom_mapper() {
    let src = r#"
        game "Test" { mapper: UxROM }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.game.mapper, Mapper::UxROM);
}

#[test]
fn parse_mmc3_mapper() {
    let src = r#"
        game "Test" { mapper: MMC3 }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.game.mapper, Mapper::MMC3);
}

#[test]
fn parse_semicolon_separators() {
    // `;` should act as an optional statement separator so short
    // statements can share a line.
    let src = r#"
        game "Test" { mapper: NROM }
        var a: u8 = 0
        var b: u8 = 0
        on frame {
            a += 1; b += 2
            if button.a { a -= 1; b -= 1 }
            wait_frame
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    // Top-level statements: two assigns + if + wait_frame
    assert_eq!(frame.statements.len(), 4);
}

#[test]
fn parse_match_statement() {
    let src = r#"
        game "Test" { mapper: NROM }
        enum State { Title, Playing, GameOver }
        var s: u8 = Title
        on frame {
            match s {
                Title => { s = Playing }
                Playing => { s = GameOver }
                _ => {}
            }
            wait_frame
        }
        start Main
    "#;
    // Match desugars to an If, so after parsing the first statement
    // inside the frame handler should be an If with two elifs and an
    // else.
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(_, _, elifs, else_block, _) => {
            assert_eq!(elifs.len(), 1, "expected 1 else-if, got {elifs:?}");
            assert!(else_block.is_some(), "expected an else block for `_`");
        }
        _ => panic!("expected If, got {:?}", frame.statements[0]),
    }
}

#[test]
fn parse_for_loop() {
    let src = r#"
        game "Test" { mapper: NROM }
        var sum: u8 = 0
        on frame {
            for i in 0..10 {
                sum += i
            }
            wait_frame
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert!(
        matches!(&frame.statements[0], Statement::For { var, .. } if var == "i"),
        "expected for loop, got {:?}",
        frame.statements[0]
    );
}

#[test]
fn parse_for_loop_with_const_bounds() {
    let src = r#"
        game "Test" { mapper: NROM }
        const START: u8 = 5
        const END: u8 = 15
        var total: u8 = 0
        on frame {
            for n in START..END { total += n }
            wait_frame
        }
        start Main
    "#;
    parse_ok(src);
}

#[test]
fn parse_audio_statements() {
    let src = r#"
        game "Audio" { mapper: NROM }
        on frame {
            play JumpSfx
            start_music MainTheme
            stop_music
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert!(matches!(frame.statements[0], Statement::Play(ref n, _) if n == "JumpSfx"));
    assert!(matches!(
        frame.statements[1],
        Statement::StartMusic(ref n, _) if n == "MainTheme"
    ));
    assert!(matches!(frame.statements[2], Statement::StopMusic(_)));
}

#[test]
fn parse_struct_decl() {
    let src = r#"
        game "Test" { mapper: NROM }
        struct Vec2 { x: u8, y: u8 }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.structs.len(), 1);
    assert_eq!(prog.structs[0].name, "Vec2");
    assert_eq!(prog.structs[0].fields.len(), 2);
    assert_eq!(prog.structs[0].fields[0].name, "x");
    assert_eq!(prog.structs[0].fields[1].name, "y");
}

#[test]
fn parse_struct_literal_expr() {
    let src = r#"
        game "Test" { mapper: NROM }
        struct Vec2 { x: u8, y: u8 }
        var pos: Vec2 = Vec2 { x: 10, y: 20 }
        on frame {
            pos = Vec2 { x: 1, y: 2 }
        }
        start Main
    "#;
    parse_ok(src);
}

#[test]
fn parse_struct_literal_in_if_condition_must_be_paren() {
    // `if x == Vec2 { ... }` is ambiguous: the `{` could be the if
    // block or the start of a struct literal. Without parens, the
    // parser should treat the struct literal fields as the if body.
    // This test just asserts the parser doesn't crash and doesn't
    // misinterpret the condition.
    let src = r#"
        game "Test" { mapper: NROM }
        struct Vec2 { x: u8, y: u8 }
        var pos: Vec2
        on frame {
            if pos.x == 5 { pos = Vec2 { x: 0, y: 0 } }
        }
        start Main
    "#;
    parse_ok(src);
}

#[test]
fn parse_struct_field_access_expr() {
    let src = r#"
        game "Test" { mapper: NROM }
        struct Vec2 { x: u8, y: u8 }
        var pos: Vec2
        on frame {
            pos.x = 10
            pos.y = pos.x
        }
        start Main
    "#;
    parse_ok(src);
}

#[test]
fn parse_enum_decl() {
    let src = r#"
        game "Test" { mapper: NROM }
        enum Direction { Up, Down, Left, Right }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.enums.len(), 1);
    assert_eq!(prog.enums[0].name, "Direction");
    assert_eq!(prog.enums[0].variants.len(), 4);
    assert_eq!(prog.enums[0].variants[0].0, "Up");
    assert_eq!(prog.enums[0].variants[3].0, "Right");
}

#[test]
fn parse_enum_trailing_comma_optional() {
    let src = r#"
        game "Test" { mapper: NROM }
        enum Flag { On, Off, }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.enums[0].variants.len(), 2);
}

#[test]
fn parse_multiple_start_declarations_error() {
    let src = r#"
        game "Test" { mapper: NROM }
        state A { on frame { wait_frame } }
        state B { on frame { wait_frame } }
        start A
        start B
    "#;
    let errors = parse_err(src);
    assert!(
        errors.contains(&crate::errors::ErrorCode::E0505),
        "duplicate start should produce E0505, got: {errors:?}"
    );
}

#[test]
fn parse_on_scanline_handler() {
    let src = r#"
        game "Test" { mapper: MMC3 }
        state Main {
            on frame { wait_frame }
            on scanline(120) { scroll(0, 0) }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let state = prog.states.iter().find(|s| s.name == "Main").unwrap();
    assert_eq!(state.on_scanline.len(), 1);
    assert_eq!(state.on_scanline[0].0, 120);
}

#[test]
fn parse_multiple_on_scanline_handlers() {
    let src = r#"
        game "Test" { mapper: MMC3 }
        state Main {
            on frame { wait_frame }
            on scanline(64) { scroll(0, 0) }
            on scanline(192) { scroll(8, 0) }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let state = prog.states.iter().find(|s| s.name == "Main").unwrap();
    assert_eq!(state.on_scanline.len(), 2);
    assert_eq!(state.on_scanline[0].0, 64);
    assert_eq!(state.on_scanline[1].0, 192);
}

#[test]
fn parse_bank_decl() {
    let src = r#"
        game "Test" { mapper: NROM }
        bank Level1Data: prg
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.banks.len(), 1);
    assert_eq!(prog.banks[0].name, "Level1Data");
    assert_eq!(prog.banks[0].bank_type, BankType::Prg);
}

// ── Edge cases and regression tests ──

#[test]
fn draw_followed_by_statement() {
    // Regression: draw keyword-arg parser used to consume the next statement's
    // identifier as a keyword arg, causing "expected ':'" errors.
    let src = r#"
        game "T" { mapper: NROM }
        var i: u8 = 0
        on frame {
            draw Sprite at: (0, 0)
            i += 1
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 2); // draw + assignment
}

#[test]
fn draw_with_frame_followed_by_statement() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 0
        on frame {
            draw Sprite at: (0, 0) frame: 1
            x = 5
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 2);
}

#[test]
fn nested_if_else_chain() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x == 0 {
                x = 1
            } else if x == 1 {
                x = 2
            } else if x == 2 {
                x = 3
            } else {
                x = 0
            }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(_, _, else_ifs, else_block, _) => {
            assert_eq!(else_ifs.len(), 2);
            assert!(else_block.is_some());
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn deeply_nested_blocks() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x == 0 {
                if x == 0 {
                    if x == 0 {
                        if x == 0 {
                            x = 1
                        }
                    }
                }
            }
        }
        start Main
    "#;
    parse_ok(src); // should not stack overflow
}

#[test]
fn empty_function_body() {
    let src = r#"
        game "T" { mapper: NROM }
        fun noop() {}
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.functions.len(), 1);
    assert!(prog.functions[0].body.statements.is_empty());
}

#[test]
fn empty_state_handlers() {
    let src = r#"
        game "T" { mapper: NROM }
        state Main {
            on enter {}
            on exit {}
            on frame {}
        }
        start Main
    "#;
    let prog = parse_ok(src);
    assert!(prog.states[0]
        .on_enter
        .as_ref()
        .unwrap()
        .statements
        .is_empty());
    assert!(prog.states[0]
        .on_exit
        .as_ref()
        .unwrap()
        .statements
        .is_empty());
    assert!(prog.states[0]
        .on_frame
        .as_ref()
        .unwrap()
        .statements
        .is_empty());
}

#[test]
fn button_start_in_condition() {
    // "start" is a keyword but valid as a button name
    let src = r#"
        game "T" { mapper: NROM }
        on frame {
            if button.start { wait_frame }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(Expr::ButtonRead(_, button, _), _, _, _, _) => {
            assert_eq!(button, "start");
        }
        other => panic!("expected if with button.start, got {other:?}"),
    }
}

#[test]
fn button_select_in_condition() {
    let src = r#"
        game "T" { mapper: NROM }
        on frame {
            if button.select { wait_frame }
        }
        start Main
    "#;
    parse_ok(src); // "select" is a keyword too — must parse as button name
}

#[test]
fn multiple_draws_in_sequence() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 0
        on frame {
            draw A at: (0, 0)
            draw B at: (10, 10)
            draw C at: (20, 20)
            x += 1
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 4); // 3 draws + 1 assignment
}

#[test]
fn parser_no_panic_on_garbage() {
    // The parser should never panic, only return errors.
    let garbage_inputs = [
        "",
        "{}{}{}",
        "game",
        "game \"\"",
        "game \"\" {",
        "game \"\" { mapper: }",
        "var : = 0",
        "fun () {}",
        "if if if",
        "while { }",
        "draw",
        "draw X",
        "draw X at:",
        "draw X at: (",
        "draw X at: (0",
        "draw X at: (0,",
        "draw X at: (0, 0",
        "sprite {",
        "palette { colors: }",
        "[[[",
        "))))",
        "0x 0b",
        "\"unterminated",
        "/* no block comments */",
        "!@#$%",
        &"x ".repeat(1000), // long input
    ];
    for input in &garbage_inputs {
        let _ = parse(input); // must not panic
    }
}

#[test]
fn lexer_no_panic_on_garbage() {
    use crate::lexer::lex;
    let garbage_inputs = [
        "\0\0\0",
        "\x01\x02\x03",
        "\"\\",
        "0x",
        "0b",
        "99999999999999999999",
        "0xFFFFFFFFFF",
        &"a".repeat(10000),
        "!!!!!!",
        "\t\r\n \t\r\n",
    ];
    for input in &garbage_inputs {
        let _ = lex(input); // must not panic
    }
}

// ── Code review regression tests ──

#[test]
fn unterminated_call_args_no_hang() {
    // Regression: parser used to infinite-loop on missing RParen in call args
    let _ = parse(r#"game "T" { mapper: NROM } on frame { foo(1, 2 } start Main"#);
    let _ = parse(r#"game "T" { mapper: NROM } var x: u8 = foo(1"#);
    // Must return (possibly with errors), not hang
}

#[test]
fn array_index_compound_assign() {
    // Regression: parse_assign_op was missing &= |= ^= for array elements
    let src = r#"
        game "T" { mapper: NROM }
        var buf: u8[4] = [0, 0, 0, 0]
        on frame {
            buf[0] &= 0x0F
            buf[1] |= 0x80
            buf[2] ^= 0xFF
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 3);
}

#[test]
fn parse_p1_button_read() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if p1.button.a { x += 1 }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(Expr::ButtonRead(Some(Player::P1), button, _), _, _, _, _) => {
            assert_eq!(button, "a");
        }
        other => panic!("expected P1 button read, got {other:?}"),
    }
}

#[test]
fn parse_p2_button_read() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if p2.button.start { x += 1 }
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::If(Expr::ButtonRead(Some(Player::P2), button, _), _, _, _, _) => {
            assert_eq!(button, "start");
        }
        other => panic!("expected P2 button read, got {other:?}"),
    }
}

#[test]
fn parse_shift_assign_operators() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 1
        on frame {
            x <<= 1
            x >>= 1
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 2);
}

#[test]
fn parse_debug_log() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 0
        on frame {
            debug.log(x)
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::DebugLog(args, _) => assert_eq!(args.len(), 1),
        other => panic!("expected DebugLog, got {other:?}"),
    }
}

#[test]
fn parse_debug_log_multiple_args() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 0
        var y: u8 = 0
        on frame {
            debug.log(x, y, 42)
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    match &frame.statements[0] {
        Statement::DebugLog(args, _) => assert_eq!(args.len(), 3),
        other => panic!("expected DebugLog, got {other:?}"),
    }
}

#[test]
fn parse_debug_assert() {
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 0
        on frame {
            debug.assert(x == 0)
        }
        start Main
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert!(matches!(frame.statements[0], Statement::DebugAssert(..)));
}
