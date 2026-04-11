#[cfg(test)]
mod tests;

use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};

/// PPU register addresses
const PPU_CTRL: u16 = 0x2000;
const PPU_MASK: u16 = 0x2001;
const PPU_STATUS: u16 = 0x2002;
const OAM_ADDR: u16 = 0x2003;
const OAM_DMA: u16 = 0x4014;
const APU_STATUS: u16 = 0x4015;
const JOY1: u16 = 0x4016;
const APU_FRAME: u16 = 0x4017;

/// Zero-page locations used by the runtime.
pub const ZP_FRAME_FLAG: u8 = 0x00;
pub const ZP_INPUT_P1: u8 = 0x01;

/// Generate the NES hardware initialization sequence.
/// This runs at RESET and sets up the hardware before user code.
pub fn gen_init() -> Vec<Instruction> {
    let mut out = Vec::new();

    // Disable IRQs and set decimal mode off
    out.push(Instruction::implied(SEI));
    out.push(Instruction::implied(CLD));

    // Disable APU frame counter IRQ
    out.push(Instruction::new(LDX, AM::Immediate(0x40)));
    out.push(Instruction::new(STX, AM::Absolute(APU_FRAME)));

    // Set up stack at $01FF
    out.push(Instruction::new(LDX, AM::Immediate(0xFF)));
    out.push(Instruction::implied(TXS));

    // Disable PPU rendering
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::Absolute(PPU_CTRL)));
    out.push(Instruction::new(STA, AM::Absolute(PPU_MASK)));

    // Disable DMC IRQs
    out.push(Instruction::new(STA, AM::Absolute(APU_STATUS)));

    // Wait for first vblank
    // vblankwait1:
    out.push(Instruction::new(NOP, AM::Label("__vblankwait1".into())));
    out.push(Instruction::new(BIT, AM::Absolute(PPU_STATUS)));
    out.push(Instruction::new(
        BPL,
        AM::LabelRelative("__vblankwait1".into()),
    ));

    // Clear RAM ($0000-$07FF)
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(LDX, AM::Immediate(0x00)));
    out.push(Instruction::new(NOP, AM::Label("__clrmem".into())));
    out.push(Instruction::new(STA, AM::AbsoluteX(0x0000)));
    out.push(Instruction::new(STA, AM::AbsoluteX(0x0100)));
    // OAM shadow: fill with $FE (hide sprites off-screen)
    out.push(Instruction::new(LDA, AM::Immediate(0xFE)));
    out.push(Instruction::new(STA, AM::AbsoluteX(0x0200)));
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::AbsoluteX(0x0300)));
    out.push(Instruction::new(STA, AM::AbsoluteX(0x0400)));
    out.push(Instruction::new(STA, AM::AbsoluteX(0x0500)));
    out.push(Instruction::new(STA, AM::AbsoluteX(0x0600)));
    out.push(Instruction::new(STA, AM::AbsoluteX(0x0700)));
    out.push(Instruction::implied(INX));
    out.push(Instruction::new(BNE, AM::LabelRelative("__clrmem".into())));

    // Wait for second vblank
    out.push(Instruction::new(NOP, AM::Label("__vblankwait2".into())));
    out.push(Instruction::new(BIT, AM::Absolute(PPU_STATUS)));
    out.push(Instruction::new(
        BPL,
        AM::LabelRelative("__vblankwait2".into()),
    ));

    // Enable PPU (sprites from pattern table 0, enable NMI)
    out.push(Instruction::new(LDA, AM::Immediate(0x80))); // enable NMI
    out.push(Instruction::new(STA, AM::Absolute(PPU_CTRL)));
    out.push(Instruction::new(LDA, AM::Immediate(0x10))); // show sprites
    out.push(Instruction::new(STA, AM::Absolute(PPU_MASK)));

    out
}

/// Generate the NMI handler.
/// Called every vblank by the NES hardware.
pub fn gen_nmi() -> Vec<Instruction> {
    let mut out = Vec::new();

    // Save registers
    out.push(Instruction::implied(PHA));
    out.push(Instruction::implied(TXA));
    out.push(Instruction::implied(PHA));
    out.push(Instruction::implied(TYA));
    out.push(Instruction::implied(PHA));

    // OAM DMA — transfer sprite data from $0200
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::Absolute(OAM_ADDR)));
    out.push(Instruction::new(LDA, AM::Immediate(0x02)));
    out.push(Instruction::new(STA, AM::Absolute(OAM_DMA)));

    // Read controller 1
    out.push(Instruction::new(LDA, AM::Immediate(0x01)));
    out.push(Instruction::new(STA, AM::Absolute(JOY1)));
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::Absolute(JOY1)));

    // Read 8 button bits into ZP_INPUT_P1
    out.push(Instruction::new(LDX, AM::Immediate(0x08)));
    out.push(Instruction::new(NOP, AM::Label("__read_input".into())));
    out.push(Instruction::new(LDA, AM::Absolute(JOY1)));
    out.push(Instruction::new(LSR, AM::Accumulator));
    out.push(Instruction::new(ROL, AM::ZeroPage(ZP_INPUT_P1)));
    out.push(Instruction::implied(DEX));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__read_input".into()),
    ));

    // Set frame-ready flag
    out.push(Instruction::new(LDA, AM::Immediate(0x01)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_FRAME_FLAG)));

    // Restore registers
    out.push(Instruction::implied(PLA));
    out.push(Instruction::implied(TAY));
    out.push(Instruction::implied(PLA));
    out.push(Instruction::implied(TAX));
    out.push(Instruction::implied(PLA));

    // Return from interrupt
    out.push(Instruction::implied(RTI));

    out
}

/// Generate the IRQ handler (just RTI for now).
pub fn gen_irq() -> Vec<Instruction> {
    vec![Instruction::implied(RTI)]
}
