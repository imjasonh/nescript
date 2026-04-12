use super::*;
use crate::analyzer;
use crate::asm::{AddressingMode as AM, Opcode::*};
use crate::parser;

fn compile_to_instructions(src: &str) -> Vec<Instruction> {
    let (prog, diags) = parser::parse(src);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let prog = prog.unwrap();
    let analysis = analyzer::analyze(&prog);
    assert!(
        analysis.diagnostics.iter().all(|d| !d.is_error()),
        "analysis errors: {:?}",
        analysis.diagnostics
    );

    let codegen = CodeGen::new(&analysis.var_allocations, &prog.constants);
    codegen.generate(&prog)
}

fn has_instruction(instructions: &[Instruction], opcode: crate::asm::Opcode, mode: &AM) -> bool {
    instructions
        .iter()
        .any(|i| i.opcode == opcode && i.mode == *mode)
}

#[test]
fn codegen_var_init() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 128
        on frame { wait_frame }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // Should have LDA #128 and STA to zero page
    assert!(has_instruction(&insts, LDA, &AM::Immediate(128)));
}

#[test]
fn codegen_plus_assign() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        on frame { px += 2 }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // Should have CLC, ADC #2
    assert!(has_instruction(&insts, CLC, &AM::Implied));
    assert!(has_instruction(&insts, ADC, &AM::Immediate(2)));
}

#[test]
fn codegen_minus_assign() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 100
        on frame { px -= 1 }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    assert!(has_instruction(&insts, SEC, &AM::Implied));
    assert!(has_instruction(&insts, SBC, &AM::Immediate(1)));
}

#[test]
fn codegen_button_check() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 0
        on frame {
            if button.right { px += 1 }
        }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // Should read controller input and AND with right button mask (0x01)
    assert!(has_instruction(&insts, AND, &AM::Immediate(0x01)));
}

#[test]
fn codegen_draw_sprite() {
    let src = r#"
        game "Test" { mapper: NROM }
        var px: u8 = 64
        var py: u8 = 64
        on frame {
            draw Smiley at: (px, py)
        }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // Should write to OAM buffer at $0200-$0203
    assert!(has_instruction(&insts, STA, &AM::Absolute(0x0200))); // Y
    assert!(has_instruction(&insts, STA, &AM::Absolute(0x0201))); // tile
    assert!(has_instruction(&insts, STA, &AM::Absolute(0x0202))); // attr
    assert!(has_instruction(&insts, STA, &AM::Absolute(0x0203))); // X
}

#[test]
fn codegen_const_usage() {
    let src = r#"
        game "Test" { mapper: NROM }
        const SPEED: u8 = 2
        var px: u8 = 0
        on frame { px += SPEED }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // Constant should be inlined as immediate
    assert!(has_instruction(&insts, ADC, &AM::Immediate(2)));
}

#[test]
fn codegen_main_loop_structure() {
    let src = r#"
        game "Test" { mapper: NROM }
        on frame { wait_frame }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // Should have JMP back to loop start
    let has_jmp = insts.iter().any(|i| {
        i.opcode == JMP && matches!(&i.mode, AM::Label(l) if l.starts_with("__main_loop"))
    });
    assert!(has_jmp, "should have JMP to main loop");
}

#[test]
fn codegen_comparison() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 0
        on frame {
            if x == 10 { x = 0 }
        }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    assert!(has_instruction(&insts, CMP, &AM::ZeroPage(0x02)));
}

#[test]
fn codegen_array_index_read() {
    let src = r#"
        game "Test" { mapper: NROM }
        var arr: u8[4] = [1, 2, 3, 4]
        var idx: u8 = 0
        var result: u8 = 0
        on frame {
            result = arr[idx]
        }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // Reading arr[idx] should use TAX + LDA,X
    assert!(has_instruction(&insts, TAX, &AM::Implied));
}

#[test]
fn codegen_array_index_write() {
    let src = r#"
        game "Test" { mapper: NROM }
        var arr: u8[4] = [1, 2, 3, 4]
        var idx: u8 = 0
        on frame {
            arr[idx] = 42
        }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // Writing arr[idx] = val should use TAX + STA,X
    assert!(has_instruction(&insts, TAX, &AM::Implied));
}

#[test]
fn codegen_scroll() {
    let src = r#"
        game "Test" { mapper: NROM }
        var sx: u8 = 0
        var sy: u8 = 0
        on frame {
            scroll(sx, sy)
        }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // scroll(x, y) should write to $2005 twice
    let count_2005 = insts
        .iter()
        .filter(|i| i.opcode == STA && i.mode == AM::Absolute(0x2005))
        .count();
    assert_eq!(count_2005, 2, "scroll should write to $2005 twice");
}

#[test]
fn codegen_multiply() {
    let src = r#"
        game "Test" { mapper: NROM }
        var a: u8 = 3
        var b: u8 = 5
        var result: u8 = 0
        on frame {
            result = a * b
        }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // a * b should generate JSR __multiply
    let has_jsr_multiply = insts
        .iter()
        .any(|i| i.opcode == JSR && matches!(&i.mode, AM::Label(l) if l == "__multiply"));
    assert!(has_jsr_multiply, "multiply should generate JSR __multiply");
}

#[test]
fn codegen_shift_left() {
    let src = r#"
        game "Test" { mapper: NROM }
        var x: u8 = 1
        var result: u8 = 0
        on frame {
            result = x << 1
        }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // x << 1 should generate ASL A
    assert!(
        has_instruction(&insts, ASL, &AM::Accumulator),
        "shift left should generate ASL A"
    );
}

#[test]
fn codegen_debug_log_stripped_in_release() {
    // Without --debug, debug.log should not emit any $4800 writes
    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 42
        on frame {
            debug.log(x)
        }
        start Main
    "#;
    let insts = compile_to_instructions(src);
    // Should NOT write to $4800
    let has_debug = insts
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4800));
    assert!(!has_debug, "debug.log should be stripped in release mode");
}

#[test]
fn codegen_debug_log_emits_in_debug_mode() {
    use crate::analyzer;
    use crate::parser;

    let src = r#"
        game "T" { mapper: NROM }
        var x: u8 = 42
        on frame {
            debug.log(x)
        }
        start Main
    "#;
    let (prog, _) = parser::parse(src);
    let prog = prog.unwrap();
    let analysis = analyzer::analyze(&prog);
    let codegen = CodeGen::new(&analysis.var_allocations, &prog.constants).with_debug(true);
    let insts = codegen.generate(&prog);
    // Should write to $4800 at least once
    let has_debug = insts
        .iter()
        .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4800));
    assert!(has_debug, "debug.log should write to $4800 in debug mode");
}
