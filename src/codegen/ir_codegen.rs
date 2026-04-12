//! IR-based code generator.
//!
//! Walks an `IrProgram` and produces 6502 instructions. Uses a simple
//! strategy: each IR temp is assigned a zero-page slot in the function's
//! temp region. Operations load operands from their slots into A, compute,
//! and store back. This is not efficient but is correct and easy to reason
//! about. A proper register allocator would use A/X/Y directly for short
//! live ranges.
//!
//! Zero-page layout (shared with AST codegen):
//! - `$00` frame flag
//! - `$01` input P1
//! - `$02` scratch temp
//! - `$03` `current_state`
//! - `$04-$07` function call params
//! - `$08` input P2
//! - `$09-$0F` reserved
//! - `$10+` user variables + IR temp slots
//!
//! IR temps are allocated starting at `TEMP_BASE` (`$80`), giving 128 bytes
//! (`0x80-0xFF`) for IR temp storage per function. Functions reset the
//! temp counter at entry.

use std::collections::HashMap;

use crate::analyzer::VarAllocation;
use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};
use crate::ir::{IrBasicBlock, IrFunction, IrOp, IrProgram, IrTemp, IrTerminator, VarId};

/// Base zero-page address for IR temp slots.
const TEMP_BASE: u8 = 0x80;

/// IR codegen that produces 6502 instructions from an `IrProgram`.
pub struct IrCodeGen<'a> {
    instructions: Vec<Instruction>,
    /// Map from IR `VarId` to zero-page address.
    var_addrs: HashMap<VarId, u16>,
    /// Map from `IrTemp` to zero-page slot within the current function.
    temp_slots: HashMap<IrTemp, u8>,
    /// Next available temp slot for the current function.
    next_temp_slot: u8,
    /// Sprite name to tile index mapping.
    sprite_tiles: HashMap<String, u8>,
    _allocations: &'a [VarAllocation],
}

impl<'a> IrCodeGen<'a> {
    pub fn new(allocations: &'a [VarAllocation], ir: &IrProgram) -> Self {
        // Map IR global VarIds to their allocated addresses.
        // Globals in IR are in the same order as in the analyzer, so we
        // can align them by name.
        let mut var_addrs = HashMap::new();
        for global in &ir.globals {
            if let Some(alloc) = allocations.iter().find(|a| a.name == global.name) {
                var_addrs.insert(global.var_id, alloc.address);
            }
        }
        Self {
            instructions: Vec::new(),
            var_addrs,
            temp_slots: HashMap::new(),
            next_temp_slot: 0,
            sprite_tiles: HashMap::new(),
            _allocations: allocations,
        }
    }

    #[must_use]
    pub fn with_sprites(mut self, sprites: &[crate::linker::SpriteData]) -> Self {
        for sprite in sprites {
            self.sprite_tiles
                .insert(sprite.name.clone(), sprite.tile_index);
        }
        self
    }

    fn emit(&mut self, opcode: crate::asm::Opcode, mode: AM) {
        self.instructions.push(Instruction::new(opcode, mode));
    }

    fn emit_label(&mut self, name: &str) {
        self.instructions
            .push(Instruction::new(NOP, AM::Label(name.to_string())));
    }

    /// Return the zero-page address for an IR temp, allocating a new slot
    /// if needed.
    fn temp_addr(&mut self, temp: IrTemp) -> u8 {
        if let Some(&slot) = self.temp_slots.get(&temp) {
            return slot;
        }
        let slot = TEMP_BASE + self.next_temp_slot;
        self.next_temp_slot = self.next_temp_slot.wrapping_add(1);
        self.temp_slots.insert(temp, slot);
        slot
    }

    /// Load a temp's value into A.
    fn load_temp(&mut self, temp: IrTemp) {
        let addr = self.temp_addr(temp);
        self.emit(LDA, AM::ZeroPage(addr));
    }

    /// Store A into a temp's slot.
    fn store_temp(&mut self, temp: IrTemp) {
        let addr = self.temp_addr(temp);
        self.emit(STA, AM::ZeroPage(addr));
    }

    /// Generate instructions for an entire IR program.
    /// Returns the flat list of 6502 instructions in the same order
    /// expected by the linker (variable init → main loop → function bodies).
    pub fn generate(mut self, ir: &IrProgram) -> Vec<Instruction> {
        // Emit variable initializers for globals with literal init values.
        for global in &ir.globals {
            if let Some(val) = global.init_value {
                if let Some(&addr) = self.var_addrs.get(&global.var_id) {
                    self.emit(LDA, AM::Immediate(val as u8));
                    if addr < 0x100 {
                        self.emit(STA, AM::ZeroPage(addr as u8));
                    } else {
                        self.emit(STA, AM::Absolute(addr));
                    }
                }
            }
        }

        // Emit each function body
        for func in &ir.functions {
            self.gen_function(func);
        }

        self.instructions
    }

    fn gen_function(&mut self, func: &IrFunction) {
        // Reset temp slot allocator per function
        self.temp_slots.clear();
        self.next_temp_slot = 0;

        self.emit_label(&format!("__ir_fn_{}", func.name));

        for block in &func.blocks {
            self.gen_block(block);
        }
    }

    fn gen_block(&mut self, block: &IrBasicBlock) {
        self.emit_label(&format!("__ir_blk_{}", block.label));

        for op in &block.ops {
            self.gen_op(op);
        }

        self.gen_terminator(&block.terminator);
    }

    #[allow(clippy::too_many_lines)]
    fn gen_op(&mut self, op: &IrOp) {
        match op {
            IrOp::LoadImm(dest, val) => {
                self.emit(LDA, AM::Immediate(*val));
                self.store_temp(*dest);
            }
            IrOp::LoadVar(dest, var) => {
                if let Some(&addr) = self.var_addrs.get(var) {
                    if addr < 0x100 {
                        self.emit(LDA, AM::ZeroPage(addr as u8));
                    } else {
                        self.emit(LDA, AM::Absolute(addr));
                    }
                    self.store_temp(*dest);
                }
            }
            IrOp::StoreVar(var, src) => {
                if let Some(&addr) = self.var_addrs.get(var) {
                    self.load_temp(*src);
                    if addr < 0x100 {
                        self.emit(STA, AM::ZeroPage(addr as u8));
                    } else {
                        self.emit(STA, AM::Absolute(addr));
                    }
                }
            }
            IrOp::Add(d, a, b) => {
                self.load_temp(*a);
                self.emit(CLC, AM::Implied);
                let b_addr = self.temp_addr(*b);
                self.emit(ADC, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::Sub(d, a, b) => {
                self.load_temp(*a);
                self.emit(SEC, AM::Implied);
                let b_addr = self.temp_addr(*b);
                self.emit(SBC, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::Mul(d, a, b) => {
                // Software multiply: multiplicand in A, multiplier in $02
                self.load_temp(*a);
                self.emit(PHA, AM::Implied); // Save for __multiply contract
                let b_addr = self.temp_addr(*b);
                self.emit(LDA, AM::ZeroPage(b_addr));
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(PLA, AM::Implied);
                self.emit(JSR, AM::Label("__multiply".into()));
                self.store_temp(*d);
            }
            IrOp::And(d, a, b) => {
                self.load_temp(*a);
                let b_addr = self.temp_addr(*b);
                self.emit(AND, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::Or(d, a, b) => {
                self.load_temp(*a);
                let b_addr = self.temp_addr(*b);
                self.emit(ORA, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::Xor(d, a, b) => {
                self.load_temp(*a);
                let b_addr = self.temp_addr(*b);
                self.emit(EOR, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::ShiftLeft(d, a, count) => {
                self.load_temp(*a);
                for _ in 0..*count {
                    self.emit(ASL, AM::Accumulator);
                }
                self.store_temp(*d);
            }
            IrOp::ShiftRight(d, a, count) => {
                self.load_temp(*a);
                for _ in 0..*count {
                    self.emit(LSR, AM::Accumulator);
                }
                self.store_temp(*d);
            }
            IrOp::Negate(d, src) => {
                self.load_temp(*src);
                self.emit(EOR, AM::Immediate(0xFF));
                self.emit(CLC, AM::Implied);
                self.emit(ADC, AM::Immediate(1));
                self.store_temp(*d);
            }
            IrOp::Complement(d, src) => {
                self.load_temp(*src);
                self.emit(EOR, AM::Immediate(0xFF));
                self.store_temp(*d);
            }
            IrOp::CmpEq(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::Eq),
            IrOp::CmpNe(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::Ne),
            IrOp::CmpLt(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::Lt),
            IrOp::CmpGt(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::Gt),
            IrOp::CmpLtEq(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::LtEq),
            IrOp::CmpGtEq(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::GtEq),
            IrOp::ArrayLoad(dest, var, idx) => {
                if let Some(&base_addr) = self.var_addrs.get(var) {
                    self.load_temp(*idx);
                    self.emit(TAX, AM::Implied);
                    if base_addr < 0x100 {
                        self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                    } else {
                        self.emit(LDA, AM::AbsoluteX(base_addr));
                    }
                    self.store_temp(*dest);
                }
            }
            IrOp::ArrayStore(var, idx, val) => {
                if let Some(&base_addr) = self.var_addrs.get(var) {
                    self.load_temp(*idx);
                    self.emit(TAX, AM::Implied);
                    self.load_temp(*val);
                    if base_addr < 0x100 {
                        self.emit(STA, AM::ZeroPageX(base_addr as u8));
                    } else {
                        self.emit(STA, AM::AbsoluteX(base_addr));
                    }
                }
            }
            IrOp::Call(dest, name, args) => {
                for (i, arg) in args.iter().enumerate() {
                    self.load_temp(*arg);
                    self.emit(STA, AM::ZeroPage(0x04 + i as u8));
                }
                self.emit(JSR, AM::Label(format!("__fn_{name}")));
                if let Some(d) = dest {
                    // Return value is in A
                    self.store_temp(*d);
                }
            }
            IrOp::DrawSprite {
                sprite_name,
                x,
                y,
                frame,
            } => {
                // Writes to OAM slot 0 for IR codegen (simple, single sprite).
                // Multi-OAM would require a slot counter like the AST codegen.
                self.load_temp(*y);
                self.emit(STA, AM::Absolute(0x0200));
                if let Some(f) = frame {
                    self.load_temp(*f);
                } else if let Some(&tile) = self.sprite_tiles.get(sprite_name) {
                    self.emit(LDA, AM::Immediate(tile));
                } else {
                    self.emit(LDA, AM::Immediate(0));
                }
                self.emit(STA, AM::Absolute(0x0201));
                self.emit(LDA, AM::Immediate(0));
                self.emit(STA, AM::Absolute(0x0202));
                self.load_temp(*x);
                self.emit(STA, AM::Absolute(0x0203));
            }
            IrOp::ReadInput(dest) => {
                self.emit(LDA, AM::ZeroPage(0x01)); // ZP_INPUT_P1
                self.store_temp(*dest);
            }
            IrOp::WaitFrame => {
                // Poll frame flag at $00 until nonzero, then clear it
                let wait_label = format!("__ir_wait_{}", self.instructions.len());
                self.emit_label(&wait_label);
                self.emit(LDA, AM::ZeroPage(0x00));
                self.emit(BEQ, AM::LabelRelative(wait_label));
                self.emit(LDA, AM::Immediate(0));
                self.emit(STA, AM::ZeroPage(0x00));
            }
            IrOp::Transition(name) => {
                // Write state index 0 as a placeholder — the AST codegen
                // does the real state index mapping. For IR codegen demo
                // purposes we emit a no-op transition here.
                let _ = name;
            }
            IrOp::SourceLoc(_) => {
                // No code for source location markers
            }
        }
    }

    fn gen_cmp(&mut self, dest: IrTemp, a: IrTemp, b: IrTemp, kind: CmpKind) {
        self.load_temp(a);
        let b_addr = self.temp_addr(b);
        self.emit(CMP, AM::ZeroPage(b_addr));

        let true_label = format!("__ir_cmp_t_{}", self.instructions.len());
        let end_label = format!("__ir_cmp_e_{}", self.instructions.len());

        match kind {
            CmpKind::Eq => self.emit(BEQ, AM::LabelRelative(true_label.clone())),
            CmpKind::Ne => self.emit(BNE, AM::LabelRelative(true_label.clone())),
            CmpKind::Lt => self.emit(BCC, AM::LabelRelative(true_label.clone())),
            CmpKind::GtEq => self.emit(BCS, AM::LabelRelative(true_label.clone())),
            CmpKind::Gt => {
                // > : not equal AND carry set
                self.emit(BEQ, AM::LabelRelative(end_label.clone()));
                self.emit(BCS, AM::LabelRelative(true_label.clone()));
            }
            CmpKind::LtEq => {
                // <= : equal OR carry clear
                self.emit(BEQ, AM::LabelRelative(true_label.clone()));
                self.emit(BCC, AM::LabelRelative(true_label.clone()));
            }
        }
        // False path
        self.emit(LDA, AM::Immediate(0));
        self.emit(JMP, AM::Label(end_label.clone()));
        // True path
        self.emit_label(&true_label);
        self.emit(LDA, AM::Immediate(1));
        self.emit_label(&end_label);
        self.store_temp(dest);
    }

    fn gen_terminator(&mut self, terminator: &IrTerminator) {
        match terminator {
            IrTerminator::Jump(label) => {
                self.emit(JMP, AM::Label(format!("__ir_blk_{label}")));
            }
            IrTerminator::Branch(cond, true_label, false_label) => {
                self.load_temp(*cond);
                // BNE true; JMP false
                self.emit(BNE, AM::LabelRelative(format!("__ir_blk_{true_label}")));
                self.emit(JMP, AM::Label(format!("__ir_blk_{false_label}")));
            }
            IrTerminator::Return(value) => {
                if let Some(v) = value {
                    self.load_temp(*v);
                }
                self.emit(RTS, AM::Implied);
            }
            IrTerminator::Unreachable => {
                // Generate a BRK just in case
                self.emit(BRK, AM::Implied);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CmpKind {
    Eq,
    Ne,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer;
    use crate::ir;
    use crate::parser;

    fn lower_and_gen(source: &str) -> Vec<Instruction> {
        let (prog, _) = parser::parse(source);
        let prog = prog.unwrap();
        let analysis = analyzer::analyze(&prog);
        let ir_program = ir::lower(&prog, &analysis);
        IrCodeGen::new(&analysis.var_allocations, &ir_program).generate(&ir_program)
    }

    fn has_instruction(insts: &[Instruction], opcode: crate::asm::Opcode, mode: &AM) -> bool {
        insts.iter().any(|i| i.opcode == opcode && i.mode == *mode)
    }

    #[test]
    fn ir_codegen_minimal_program() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 42
            on frame { x = 1 }
            start Main
        "#,
        );
        // Should initialize x = 42
        assert!(has_instruction(&insts, LDA, &AM::Immediate(42)));
    }

    #[test]
    fn ir_codegen_plus_assign() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame { x += 5 }
            start Main
        "#,
        );
        // Should emit CLC + ADC for the add
        assert!(has_instruction(&insts, CLC, &AM::Implied));
        assert!(insts.iter().any(|i| i.opcode == ADC));
    }

    #[test]
    fn ir_codegen_draw_sprite() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var px: u8 = 0
            var py: u8 = 0
            on frame { draw Smiley at: (px, py) }
            start Main
        "#,
        );
        // Should write to OAM slot 0
        assert!(has_instruction(&insts, STA, &AM::Absolute(0x0200)));
        assert!(has_instruction(&insts, STA, &AM::Absolute(0x0203)));
    }

    #[test]
    fn ir_codegen_wait_frame() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            on frame { wait_frame }
            start Main
        "#,
        );
        // Should poll frame flag
        assert!(has_instruction(&insts, LDA, &AM::ZeroPage(0x00)));
    }

    #[test]
    fn ir_codegen_button_read() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame {
                if button.right { x += 1 }
            }
            start Main
        "#,
        );
        // Should read input byte
        assert!(has_instruction(&insts, LDA, &AM::ZeroPage(0x01)));
        // Should AND with mask
        assert!(insts.iter().any(|i| i.opcode == AND));
    }

    #[test]
    fn ir_codegen_while_loop() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame {
                while x < 10 { x += 1 }
            }
            start Main
        "#,
        );
        // Should emit CMP + conditional branch
        assert!(insts.iter().any(|i| i.opcode == CMP));
        assert!(insts.iter().any(|i| i.opcode == JMP || i.opcode == BNE));
    }

    #[test]
    fn ir_codegen_if_branch() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame {
                if x == 0 { x = 1 }
            }
            start Main
        "#,
        );
        // Should emit CMP + branch
        assert!(insts.iter().any(|i| i.opcode == CMP));
    }
}
