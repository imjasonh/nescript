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

/// Zero-page addresses (shared with AST codegen).
const ZP_FRAME_FLAG: u8 = 0x00;
const ZP_CURRENT_STATE: u8 = 0x03;

/// Emulator debug output port. Writes to this address are logged by
/// Mesen / fceux when the debugger is attached. Used by `debug.log`
/// and `debug.assert` when compiled with `--debug`.
const DEBUG_PORT: u16 = 0x4800;

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
    /// State name to dispatch index mapping.
    state_indices: HashMap<String, u8>,
    /// Set of function names defined in the IR program (for existence checks).
    function_names: std::collections::HashSet<String>,
    /// Next OAM slot to allocate (0-63); reset at start of each frame handler.
    next_oam_slot: u8,
    /// True while generating code inside a state frame handler.
    /// When set, `Return` terminators emit `JMP __ir_main_loop` instead of `RTS`.
    in_frame_handler: bool,
    /// When true, emit code for `debug.log` / `debug.assert`.
    /// When false, these ops are stripped entirely.
    debug_mode: bool,
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
        let function_names = ir.functions.iter().map(|f| f.name.clone()).collect();
        Self {
            instructions: Vec::new(),
            var_addrs,
            temp_slots: HashMap::new(),
            next_temp_slot: 0,
            sprite_tiles: HashMap::new(),
            state_indices: HashMap::new(),
            function_names,
            next_oam_slot: 0,
            in_frame_handler: false,
            debug_mode: false,
            _allocations: allocations,
        }
    }

    /// Enable debug-mode code generation. When set, `debug.log` and
    /// `debug.assert` emit runtime code; otherwise they are stripped.
    #[must_use]
    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug_mode = debug;
        self
    }

    fn function_exists(&self, name: &str) -> bool {
        self.function_names.contains(name)
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
    ///
    /// Layout:
    /// 1. Variable initializers (globals with literal init values)
    /// 2. `current_state` initialization to start state index
    /// 3. Main dispatch loop (wait vblank, then `JMP` to state's frame handler)
    /// 4. State frame handlers (each ends with `JMP` to main loop)
    /// 5. User function bodies (end with `RTS`)
    pub fn generate(mut self, ir: &IrProgram) -> Vec<Instruction> {
        // Populate state indices
        for (i, name) in ir.states.iter().enumerate() {
            self.state_indices.insert(name.clone(), i as u8);
        }

        // 1. Variable initializers
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

        // 2. Initialize current_state to start state index and call
        // the start state's on_enter handler (if any).
        if let Some(&start_idx) = self.state_indices.get(&ir.start_state) {
            self.emit(LDA, AM::Immediate(start_idx));
            self.emit(STA, AM::ZeroPage(ZP_CURRENT_STATE));
            let enter_fn = format!("{}_enter", ir.start_state);
            if self.function_exists(&enter_fn) {
                self.emit(JSR, AM::Label(format!("__ir_fn_{enter_fn}")));
            }
        }

        // 3. Main dispatch loop
        let main_loop = "__ir_main_loop".to_string();
        self.emit_label(&main_loop);

        // Wait for vblank flag
        let wait_label = "__ir_wait_vblank".to_string();
        self.emit_label(&wait_label);
        self.emit(LDA, AM::ZeroPage(ZP_FRAME_FLAG));
        self.emit(BEQ, AM::LabelRelative(wait_label));
        // Clear the flag
        self.emit(LDA, AM::Immediate(0));
        self.emit(STA, AM::ZeroPage(ZP_FRAME_FLAG));

        // Dispatch on current_state using CMP + BNE + JMP trampoline
        self.emit(LDA, AM::ZeroPage(ZP_CURRENT_STATE));
        for (i, state_name) in ir.states.iter().enumerate() {
            let frame_handler = format!("{state_name}_frame");
            // Only dispatch if the state has a frame handler
            if ir.functions.iter().any(|f| f.name == frame_handler) {
                let skip_label = format!("__ir_disp_skip_{i}");
                self.emit(CMP, AM::Immediate(i as u8));
                self.emit(BNE, AM::LabelRelative(skip_label.clone()));
                self.emit(JMP, AM::Label(format!("__ir_fn_{frame_handler}")));
                self.emit_label(&skip_label);
            }
        }
        self.emit(JMP, AM::Label(main_loop));

        // 4. Emit each function body (state handlers + user functions)
        for func in &ir.functions {
            self.gen_function(func);
        }

        self.instructions
    }

    fn gen_function(&mut self, func: &IrFunction) {
        // Reset temp slot allocator per function
        self.temp_slots.clear();
        self.next_temp_slot = 0;
        // Reset OAM slot counter per frame handler
        self.in_frame_handler = func.name.ends_with("_frame");
        if self.in_frame_handler {
            self.next_oam_slot = 0;
        }

        self.emit_label(&format!("__ir_fn_{}", func.name));

        for block in &func.blocks {
            self.gen_block(block);
        }

        self.in_frame_handler = false;
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
                // Allocate an OAM slot from $0200-$02FF. Silently drop
                // draws beyond slot 63 (OAM is full).
                if self.next_oam_slot >= 64 {
                    return;
                }
                let slot = self.next_oam_slot;
                self.next_oam_slot = self.next_oam_slot.wrapping_add(1);
                let base = 0x0200 + u16::from(slot) * 4;

                // Y position at base+0
                self.load_temp(*y);
                self.emit(STA, AM::Absolute(base));

                // Tile index at base+1 — frame override, sprite lookup, or 0
                if let Some(f) = frame {
                    self.load_temp(*f);
                } else if let Some(&tile) = self.sprite_tiles.get(sprite_name) {
                    self.emit(LDA, AM::Immediate(tile));
                } else {
                    self.emit(LDA, AM::Immediate(0));
                }
                self.emit(STA, AM::Absolute(base + 1));

                // Attributes at base+2 (always 0)
                self.emit(LDA, AM::Immediate(0));
                self.emit(STA, AM::Absolute(base + 2));

                // X position at base+3
                self.load_temp(*x);
                self.emit(STA, AM::Absolute(base + 3));
            }
            IrOp::ReadInput(dest, player) => {
                // $01 = P1 input byte, $08 = P2 input byte
                let addr = if *player == 1 { 0x08 } else { 0x01 };
                self.emit(LDA, AM::ZeroPage(addr));
                self.store_temp(*dest);
            }
            IrOp::WaitFrame => {
                // Poll frame flag at $00 until nonzero, then clear it
                let wait_label = format!("__ir_wait_{}", self.instructions.len());
                self.emit_label(&wait_label);
                self.emit(LDA, AM::ZeroPage(ZP_FRAME_FLAG));
                self.emit(BEQ, AM::LabelRelative(wait_label));
                self.emit(LDA, AM::Immediate(0));
                self.emit(STA, AM::ZeroPage(ZP_FRAME_FLAG));
            }
            IrOp::Transition(name) => {
                // Write the target state's index to current_state, then
                // call the target state's on_enter handler if it exists,
                // then JMP to main loop for the new state's frame handler.
                //
                // Note: on_exit of the current state isn't called here
                // because we don't know from an IR op alone which state
                // we're leaving. Proper on_exit support would need
                // per-state transition lowering. Future improvement.
                if let Some(&idx) = self.state_indices.get(name) {
                    self.emit(LDA, AM::Immediate(idx));
                    self.emit(STA, AM::ZeroPage(ZP_CURRENT_STATE));
                    // Call the target state's on_enter handler if defined
                    let enter_fn = format!("{name}_enter");
                    if self.function_exists(&enter_fn) {
                        self.emit(JSR, AM::Label(format!("__ir_fn_{enter_fn}")));
                    }
                    self.emit(JMP, AM::Label("__ir_main_loop".into()));
                }
            }
            IrOp::Scroll(x, y) => {
                // PPU scroll register $2005 takes two writes: X then Y
                self.load_temp(*x);
                self.emit(STA, AM::Absolute(0x2005));
                self.load_temp(*y);
                self.emit(STA, AM::Absolute(0x2005));
            }
            IrOp::DebugLog(args) => {
                if self.debug_mode {
                    for arg in args {
                        self.load_temp(*arg);
                        self.emit(STA, AM::Absolute(DEBUG_PORT));
                    }
                }
                // In release mode, stripped entirely
            }
            IrOp::DebugAssert(cond) => {
                if self.debug_mode {
                    // Load cond; if nonzero (true) skip; else halt
                    self.load_temp(*cond);
                    let pass_label = format!("__ir_assert_pass_{}", self.instructions.len());
                    self.emit(BNE, AM::LabelRelative(pass_label.clone()));
                    // Assertion failed: write marker to debug port and BRK
                    self.emit(LDA, AM::Immediate(0xFF));
                    self.emit(STA, AM::Absolute(DEBUG_PORT));
                    self.emit(BRK, AM::Implied);
                    self.emit_label(&pass_label);
                }
            }
            IrOp::InlineAsm(body) => {
                // Parse the asm body with the shared inline parser and
                // splice the resulting instructions directly into our
                // output stream. Parse errors are emitted as `BRK` so
                // the ROM still links — codegen is too late to return
                // user-facing diagnostics cleanly.
                match crate::asm::parse_inline(body) {
                    Ok(parsed) => self.instructions.extend(parsed),
                    Err(msg) => {
                        eprintln!("inline asm error: {msg}");
                        self.emit(BRK, AM::Implied);
                    }
                }
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
                // Frame handlers return to the main dispatch loop,
                // not via RTS (they aren't called via JSR).
                if self.in_frame_handler {
                    self.emit(JMP, AM::Label("__ir_main_loop".into()));
                } else {
                    self.emit(RTS, AM::Implied);
                }
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

#[cfg(test)]
mod more_tests {
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

    #[test]
    fn ir_codegen_state_dispatch_emits_main_loop() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            on frame { wait_frame }
            start Main
        "#,
        );
        // Should contain the __ir_main_loop label
        let has_main_loop = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__ir_main_loop"));
        assert!(has_main_loop, "IR codegen should emit main loop label");
    }

    #[test]
    fn ir_codegen_multi_oam_uses_sequential_slots() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var a: u8 = 10
            var b: u8 = 20
            on frame {
                draw First at: (a, a)
                draw Second at: (b, b)
            }
            start Main
        "#,
        );
        // Should write to OAM slot 0 ($0200) and slot 1 ($0204)
        let writes_slot0 = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x0200));
        let writes_slot1 = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x0204));
        assert!(writes_slot0, "first draw should use OAM slot 0");
        assert!(writes_slot1, "second draw should use OAM slot 1");
    }

    #[test]
    fn ir_codegen_transition_writes_state_index() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            state A {
                on frame { transition B }
            }
            state B {
                on frame { wait_frame }
            }
            start A
        "#,
        );
        // Should write state index 1 (B is second state) to ZP $03
        let has_store_state = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::ZeroPage(0x03));
        assert!(has_store_state, "transition should write to current_state");
    }

    #[test]
    fn ir_codegen_scroll_writes_ppu_register() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var sx: u8 = 0
            var sy: u8 = 0
            on frame { scroll(sx, sy) }
            start Main
        "#,
        );
        // Both X and Y scroll values should be written to $2005
        let scroll_writes = insts
            .iter()
            .filter(|i| i.opcode == STA && i.mode == AM::Absolute(0x2005))
            .count();
        assert_eq!(scroll_writes, 2, "scroll should emit two STA $2005 writes");
    }

    fn lower_and_gen_debug(source: &str) -> Vec<Instruction> {
        let (prog, _) = parser::parse(source);
        let prog = prog.unwrap();
        let analysis = analyzer::analyze(&prog);
        let ir_program = ir::lower(&prog, &analysis);
        IrCodeGen::new(&analysis.var_allocations, &ir_program)
            .with_debug(true)
            .generate(&ir_program)
    }

    #[test]
    fn ir_codegen_debug_log_emits_in_debug_mode() {
        let insts = lower_and_gen_debug(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 42
            on frame { debug.log(x) }
            start Main
        "#,
        );
        // Should write to the debug port $4800
        let writes_debug_port = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4800));
        assert!(writes_debug_port, "debug.log should write to $4800");
    }

    #[test]
    fn ir_codegen_debug_log_stripped_in_release() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 42
            on frame { debug.log(x) }
            start Main
        "#,
        );
        // No debug port writes in release mode
        let writes_debug_port = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4800));
        assert!(
            !writes_debug_port,
            "debug.log should be stripped in release mode"
        );
    }

    #[test]
    fn ir_codegen_debug_assert_emits_in_debug_mode() {
        let insts = lower_and_gen_debug(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 42
            on frame { debug.assert(x == 42) }
            start Main
        "#,
        );
        // Should emit a BRK for the fail path
        let has_brk = insts.iter().any(|i| i.opcode == BRK);
        assert!(has_brk, "debug.assert should emit BRK on failure path");
    }
}
