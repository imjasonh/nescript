//! Peephole optimizations over the 6502 instruction stream.
//!
//! Runs after codegen but before assembly, so we can rewrite
//! `Instruction`s directly. Kept conservative to avoid breaking the
//! IR codegen's zero-page slot assumptions.

use crate::asm::{AddressingMode, Instruction, Opcode};

/// Run all peephole passes until fixed point.
pub fn optimize(instructions: &mut Vec<Instruction>) {
    loop {
        let before = instructions.len();
        remove_sta_then_lda(instructions);
        remove_lda_then_sta_same(instructions);
        if instructions.len() == before {
            break;
        }
    }
}

/// Remove `LDA addr` immediately followed by `STA addr` (same addr).
/// The store is a no-op because the byte is already there.
fn remove_lda_then_sta_same(instructions: &mut Vec<Instruction>) {
    let mut out = Vec::with_capacity(instructions.len());
    let mut i = 0;
    while i < instructions.len() {
        if i + 1 < instructions.len() {
            let a = &instructions[i];
            let b = &instructions[i + 1];
            if a.opcode == Opcode::LDA && b.opcode == Opcode::STA && a.mode == b.mode {
                // Keep the LDA (in case the value in A is used later)
                // but drop the pointless STA.
                out.push(a.clone());
                i += 2;
                continue;
            }
        }
        out.push(instructions[i].clone());
        i += 1;
    }
    *instructions = out;
}

/// Remove `STA slot` immediately followed by `LDA slot` when both
/// refer to an IR temp slot. The LDA is redundant because A already
/// holds the value we just stored.
///
/// This targets the IR codegen's store-every-temp pattern: ops that
/// produce a value into `A` then immediately store it, and the next
/// op loads it back.
fn remove_sta_then_lda(instructions: &mut Vec<Instruction>) {
    let mut out = Vec::with_capacity(instructions.len());
    let mut i = 0;
    while i < instructions.len() {
        if i + 1 < instructions.len() {
            let a = &instructions[i];
            let b = &instructions[i + 1];
            if a.opcode == Opcode::STA
                && b.opcode == Opcode::LDA
                && a.mode == b.mode
                && is_temp_slot(&a.mode)
            {
                // Keep the STA (subsequent code may read the slot),
                // drop the LDA.
                out.push(a.clone());
                i += 2;
                continue;
            }
        }
        out.push(instructions[i].clone());
        i += 1;
    }
    *instructions = out;
}

/// True if the addressing mode targets an IR temp slot ($80-$FF).
/// We restrict peephole store/load elimination to temp slots so we
/// don't accidentally merge accesses to user variables in ZP (where
/// an intervening call or IRQ could have clobbered A).
fn is_temp_slot(mode: &AddressingMode) -> bool {
    matches!(mode, AddressingMode::ZeroPage(addr) if *addr >= 0x80)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::AddressingMode as AM;
    use crate::asm::Opcode::*;

    #[test]
    fn removes_sta_then_lda_temp() {
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x80)),
            Instruction::new(LDA, AM::ZeroPage(0x80)),
            Instruction::new(CLC, AM::Implied),
        ];
        optimize(&mut insts);
        assert_eq!(insts.len(), 2);
        assert_eq!(insts[0].opcode, STA);
        assert_eq!(insts[1].opcode, CLC);
    }

    #[test]
    fn keeps_sta_then_lda_user_var() {
        // $10 is a user variable, not a temp slot — must not eliminate.
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x10)),
            Instruction::new(LDA, AM::ZeroPage(0x10)),
        ];
        optimize(&mut insts);
        assert_eq!(insts.len(), 2);
    }

    #[test]
    fn removes_lda_then_sta_same_address() {
        let mut insts = vec![
            Instruction::new(LDA, AM::ZeroPage(0x10)),
            Instruction::new(STA, AM::ZeroPage(0x10)),
            Instruction::new(CLC, AM::Implied),
        ];
        optimize(&mut insts);
        // LDA kept (value in A may be used), pointless STA removed
        assert_eq!(insts.len(), 2);
        assert_eq!(insts[0].opcode, LDA);
        assert_eq!(insts[1].opcode, CLC);
    }

    #[test]
    fn preserves_different_addresses() {
        let mut insts = vec![
            Instruction::new(STA, AM::ZeroPage(0x80)),
            Instruction::new(LDA, AM::ZeroPage(0x81)),
        ];
        optimize(&mut insts);
        assert_eq!(insts.len(), 2);
    }

    #[test]
    fn idempotent_on_optimized_code() {
        let mut insts = vec![
            Instruction::new(LDA, AM::Immediate(5)),
            Instruction::new(CLC, AM::Implied),
            Instruction::new(RTS, AM::Implied),
        ];
        let before = insts.len();
        optimize(&mut insts);
        assert_eq!(insts.len(), before);
    }
}
