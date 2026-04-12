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

#[test]
fn parse_palette_decl() {
    let src = r#"
        game "Test" { mapper: NROM }
        palette MainPal {
            colors: [0x0F, 0x00, 0x10, 0x20]
        }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.palettes.len(), 1);
    assert_eq!(prog.palettes[0].name, "MainPal");
    assert_eq!(prog.palettes[0].colors, vec![0x0F, 0x00, 0x10, 0x20]);
}

#[test]
fn parse_background_decl() {
    let src = r#"
        game "Test" { mapper: NROM }
        background TitleBg {
            chr: @chr("title.png")
        }
        on frame { wait_frame }
        start Main
    "#;
    let prog = parse_ok(src);
    assert_eq!(prog.backgrounds.len(), 1);
    assert_eq!(prog.backgrounds[0].name, "TitleBg");
    match &prog.backgrounds[0].chr_source {
        AssetSource::Chr(path) => assert_eq!(path, "title.png"),
        other => panic!("expected Chr, got {other:?}"),
    }
}

#[test]
fn parse_load_background_statement() {
    let src = r#"
        game "Test" { mapper: NROM }
        state Title {
            on frame {
                load_background TitleBg
            }
        }
        start Title
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 1);
    match &frame.statements[0] {
        Statement::LoadBackground(name, _) => assert_eq!(name, "TitleBg"),
        other => panic!("expected LoadBackground, got {other:?}"),
    }
}

#[test]
fn parse_set_palette_statement() {
    let src = r#"
        game "Test" { mapper: NROM }
        state Title {
            on frame {
                set_palette MainPal
            }
        }
        start Title
    "#;
    let prog = parse_ok(src);
    let frame = prog.states[0].on_frame.as_ref().unwrap();
    assert_eq!(frame.statements.len(), 1);
    match &frame.statements[0] {
        Statement::SetPalette(name, _) => assert_eq!(name, "MainPal"),
        other => panic!("expected SetPalette, got {other:?}"),
    }
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
