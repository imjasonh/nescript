use super::*;
use crate::analyzer;
use crate::parser;

fn lower_ok(input: &str) -> IrProgram {
    let (prog, diags) = parser::parse(input);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let prog = prog.unwrap();
    let analysis = analyzer::analyze(&prog);
    assert!(
        analysis.diagnostics.iter().all(|d| !d.is_error()),
        "analysis errors: {:?}",
        analysis.diagnostics
    );
    lower(&prog, &analysis)
}

#[test]
fn lower_minimal_program() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var px: u8 = 128
        on frame { px = 1 }
        start Main
    "#,
    );
    assert_eq!(ir.globals.len(), 1);
    assert_eq!(ir.globals[0].name, "px");
    assert_eq!(ir.globals[0].init_value, Some(128));
    // Should have at least one function (the frame handler)
    assert!(!ir.functions.is_empty());
}

#[test]
fn lower_var_assignment() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame { x = 42 }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .unwrap();
    // Should have a StoreVar op
    let has_store = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::StoreVar(..)));
    assert!(has_store, "should emit StoreVar for assignment");
}

#[test]
fn lower_plus_assign() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame { x += 5 }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .unwrap();
    let has_add = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::Add(..)));
    assert!(has_add, "should emit Add for += operator");
}

#[test]
fn lower_if_creates_branch() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x == 0 { x = 1 }
        }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .unwrap();
    let has_branch = frame_fn
        .blocks
        .iter()
        .any(|b| matches!(&b.terminator, IrTerminator::Branch(..)));
    assert!(
        has_branch,
        "if statement should produce a Branch terminator"
    );
}

#[test]
fn lower_while_creates_loop() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            while x < 10 { x += 1 }
        }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .unwrap();
    // A while loop needs at least 3 blocks: condition check, body, and exit
    assert!(
        frame_fn.blocks.len() >= 3,
        "while should create multiple blocks, got {}",
        frame_fn.blocks.len()
    );
}

#[test]
fn lower_button_read() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        on frame {
            if button.right { px += 1 }
        }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .unwrap();
    let has_input = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::ReadInput(_, _)));
    assert!(has_input, "button read should emit ReadInput op");
}

#[test]
fn lower_draw_sprite() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        var py: u8 = 0
        on frame { draw Smiley at: (px, py) }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .unwrap();
    let has_draw = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::DrawSprite { .. }));
    assert!(has_draw, "should emit DrawSprite op");
}

#[test]
fn lower_constants_become_immediates() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        const SPEED: u8 = 3
        var px: u8 = 0
        on frame { px += SPEED }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .unwrap();
    // SPEED should be lowered to LoadImm(_, 3)
    let has_imm3 = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::LoadImm(_, 3)));
    assert!(has_imm3, "constant should be inlined as LoadImm");
}

#[test]
fn lower_const_expressions_constant_fold() {
    // Constants may reference earlier constants and use arithmetic.
    // `B` resolves to `A + 3` = 8 at lowering time.
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        const A: u8 = 5
        const B: u8 = A + 3
        var x: u8 = B
        on frame { wait_frame }
        start Main
    "#,
    );
    let x_global = ir.globals.iter().find(|g| g.name == "x").unwrap();
    assert_eq!(x_global.init_value, Some(8));
}

#[test]
fn lower_const_bit_ops() {
    // Bitwise constant folding should work for things like defining
    // flags or masks based on other constants.
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        const FLAG_A: u8 = 1
        const FLAG_B: u8 = 2
        const BOTH: u8 = FLAG_A | FLAG_B
        var x: u8 = BOTH
        on frame { wait_frame }
        start Main
    "#,
    );
    let x_global = ir.globals.iter().find(|g| g.name == "x").unwrap();
    assert_eq!(x_global.init_value, Some(3));
}

#[test]
fn lower_multiple_states() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        state Title {
            on enter { wait_frame }
            on frame { wait_frame }
        }
        state Game {
            on frame { wait_frame }
        }
        start Title
    "#,
    );
    // Should have: Title_enter, Title_frame, Game_frame
    assert!(
        ir.functions.len() >= 3,
        "should have at least 3 functions for 2 states, got {}",
        ir.functions.len()
    );
    let names: Vec<&str> = ir.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.contains("Title_enter")),
        "should have Title_enter handler"
    );
    assert!(
        names.iter().any(|n| n.contains("Title_frame")),
        "should have Title_frame handler"
    );
    assert!(
        names.iter().any(|n| n.contains("Game_frame")),
        "should have Game_frame handler"
    );
}

#[test]
fn lower_op_count() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame { x = 1 }
        start Main
    "#,
    );
    assert!(ir.op_count() > 0, "should have some IR ops");
}

#[test]
fn lower_wait_frame() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        on frame { wait_frame }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .unwrap();
    let has_wait = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::WaitFrame));
    assert!(has_wait, "should emit WaitFrame op");
}

#[test]
fn array_literal_global_init_is_captured() {
    // Regression test: `var xs: u8[4] = [1, 2, 3, 4]` used to lose
    // its initializer because `eval_const` returns None for
    // `Expr::ArrayLiteral` and `init_value` ended up `None`. The
    // fix captures the per-element values in a new `init_array`
    // field so the IR codegen can emit one `LDA #imm; STA base+i`
    // per byte at startup.
    let ir = lower_ok(
        r#"
        game "Arr" { mapper: NROM }
        var xs: u8[4] = [1, 2, 3, 4]
        on frame { wait_frame }
        start Main
    "#,
    );
    let xs = ir
        .globals
        .iter()
        .find(|g| g.name == "xs")
        .expect("`xs` global should exist");
    assert_eq!(
        xs.init_array,
        vec![1, 2, 3, 4],
        "array literal initializer should populate init_array: {:?}",
        xs.init_array
    );
}

#[test]
fn for_loop_counter_is_registered_as_handler_local() {
    // Regression test for bug B's secondary fix: `for i in 0..N`
    // implicitly declares the counter `i`, and the lowering must
    // push it onto `current_locals` so the IR codegen can give
    // it a backing address. Without this entry, every
    // `LoadVar(i)` / `StoreVar(i)` in the desugared while loop
    // silently emitted no code (the codegen's `var_addrs` lookup
    // returned None), the counter stayed at 0, the loop spun
    // forever, and any `draw` inside the loop kept writing to
    // the first OAM slot with the index-0 array element.
    let ir = lower_ok(
        r#"
        game "ForCounter" { mapper: NROM }
        var xs: u8[4] = [1, 2, 3, 4]
        var out: u8 = 0
        on frame {
            for i in 0..4 {
                out = xs[i]
            }
        }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .expect("frame handler should exist");
    assert!(
        frame_fn.locals.iter().any(|l| l.name == "i"),
        "for-loop counter `i` should be registered as a handler local: {:?}",
        frame_fn.locals
    );
}

// Regression tests: shift / div / mod used to miscompile silently.
// `x << n` with a literal `n` always emitted ShiftLeft(..., 1) and
// `x / n` / `x % n` always emitted LoadImm(..., 0). These tests
// anchor the fixes from the code-review cleanup pass.

#[test]
fn lower_shift_left_with_literal_count_uses_that_count() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 1
        on frame { x = x << 3 }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .expect("frame handler should exist");
    let has_shift3 = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::ShiftLeft(_, _, 3)));
    assert!(
        has_shift3,
        "expected ShiftLeft with count=3, got ops: {:?}",
        frame_fn
            .blocks
            .iter()
            .flat_map(|b| &b.ops)
            .collect::<Vec<_>>()
    );
}

#[test]
fn lower_shift_right_with_variable_count_uses_runtime_variant() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 128
        var n: u8 = 2
        on frame { x = x >> n }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .expect("frame handler should exist");
    let has_shift_var = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::ShiftRightVar(..)));
    assert!(
        has_shift_var,
        "expected ShiftRightVar for runtime shift amount, got ops: {:?}",
        frame_fn
            .blocks
            .iter()
            .flat_map(|b| &b.ops)
            .collect::<Vec<_>>()
    );
}

#[test]
fn lower_divide_emits_div_op_not_load_imm_zero() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 100
        var d: u8 = 7
        var q: u8 = 0
        on frame { q = x / d }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .expect("frame handler should exist");
    let has_div = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::Div(..)));
    assert!(
        has_div,
        "expected IrOp::Div for `q = x / d`, got ops: {:?}",
        frame_fn
            .blocks
            .iter()
            .flat_map(|b| &b.ops)
            .collect::<Vec<_>>()
    );
}

#[test]
fn lower_modulo_emits_mod_op_not_load_imm_zero() {
    let ir = lower_ok(
        r#"
        game "Test" { mapper: NROM }
        var x: u8 = 17
        var d: u8 = 5
        var r: u8 = 0
        on frame { r = x % d }
        start Main
    "#,
    );
    let frame_fn = ir
        .functions
        .iter()
        .find(|f| f.name.contains("frame"))
        .expect("frame handler should exist");
    let has_mod = frame_fn
        .blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::Mod(..)));
    assert!(
        has_mod,
        "expected IrOp::Mod for `r = x % d`, got ops: {:?}",
        frame_fn
            .blocks
            .iter()
            .flat_map(|b| &b.ops)
            .collect::<Vec<_>>()
    );
}
