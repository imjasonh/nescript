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
pub const ZP_INPUT_P2: u8 = 0x08;
/// Runtime OAM cursor, incremented by 4 on every `draw` inside a
/// frame handler. The IR codegen resets this to 0 after the OAM
/// clear at the top of the handler, so each `draw` writes to the
/// next 4-byte sprite slot regardless of how many loop iterations
/// came before it. At 64 slots the u8 naturally wraps to 0 and
/// the oldest slot gets overwritten — the classic NES flicker
/// fallback.
pub const ZP_OAM_CURSOR: u8 = 0x09;
/// Pulse-1 SFX countdown frames. `play SfxName` sets this to the
/// SFX duration and writes the tone registers; the NMI audio tick
/// decrements it every frame, silencing pulse 1 when it reaches 0.
pub const ZP_SFX_COUNTER: u8 = 0x0A;
/// Pulse-2 music countdown frames. `start_music TrackName` sets
/// this to `$FF` (infinite sustain) and writes a pulse-2 tone;
/// `stop_music` zeros it and mutes pulse 2. `$FF` skips the NMI
/// decrement so music plays until explicitly stopped.
pub const ZP_MUSIC_COUNTER: u8 = 0x0B;

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

    // Disable DMC IRQs momentarily (will re-enable the square
    // channels below so `play`/`start_music` can make sound).
    out.push(Instruction::new(STA, AM::Absolute(APU_STATUS)));

    // Enable pulse 1 and pulse 2 channels for the minimal audio
    // driver. SFX runs on pulse 1, music on pulse 2. We leave
    // triangle / noise / DMC disabled — the engine is deliberately
    // simple and those channels would go unused anyway.
    out.push(Instruction::new(LDA, AM::Immediate(0x03)));
    out.push(Instruction::new(STA, AM::Absolute(APU_STATUS)));
    // Pre-silence both channels: `$30` on the volume register sets
    // constant-volume envelope with volume 0 and halts the length
    // counter, which is the canonical "silent but armed" state.
    out.push(Instruction::new(LDA, AM::Immediate(0x30)));
    out.push(Instruction::new(STA, AM::Absolute(0x4000)));
    out.push(Instruction::new(STA, AM::Absolute(0x4004)));
    // Clear sweep units so the channel tone doesn't auto-slide.
    out.push(Instruction::new(LDA, AM::Immediate(0x08)));
    out.push(Instruction::new(STA, AM::Absolute(0x4001)));
    out.push(Instruction::new(STA, AM::Absolute(0x4005)));
    // Restore the zero we need for the subsequent RAM clear below.
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));

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

    // Read 8 button bits from controller 1 ($4016) into ZP_INPUT_P1
    // and 8 button bits from controller 2 ($4017) into ZP_INPUT_P2
    // simultaneously — shift each port's carry into its ZP byte.
    out.push(Instruction::new(LDX, AM::Immediate(0x08)));
    out.push(Instruction::new(NOP, AM::Label("__read_input".into())));
    out.push(Instruction::new(LDA, AM::Absolute(JOY1)));
    out.push(Instruction::new(LSR, AM::Accumulator));
    out.push(Instruction::new(ROL, AM::ZeroPage(ZP_INPUT_P1)));
    out.push(Instruction::new(LDA, AM::Absolute(0x4017))); // JOY2
    out.push(Instruction::new(LSR, AM::Accumulator));
    out.push(Instruction::new(ROL, AM::ZeroPage(ZP_INPUT_P2)));
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

/// Zero-page locations used by multiply/divide routines.
const ZP_MUL_OPERAND: u8 = 0x02;
const ZP_MUL_RESULT_HI: u8 = 0x03;
const ZP_DIV_DIVISOR: u8 = 0x02;
const ZP_DIV_REMAINDER: u8 = 0x03;

/// Generate 8x8 -> 16 software multiply routine.
///
/// Input: A = multiplicand, zero-page $02 = multiplier
/// Output: A = result low byte, $03 = result high byte
///
/// Algorithm: shift-and-add. For each bit of the multiplier, if set,
/// add the (shifted) multiplicand to the result.
pub fn gen_multiply() -> Vec<Instruction> {
    let mut out = Vec::new();

    // Label for the subroutine entry
    out.push(Instruction::new(NOP, AM::Label("__multiply".into())));

    // Store multiplicand in $04 (working copy)
    out.push(Instruction::new(STA, AM::ZeroPage(0x04)));

    // Clear result: A (low) and $03 (high)
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_MUL_RESULT_HI)));

    // Loop counter: 8 bits
    out.push(Instruction::new(LDX, AM::Immediate(0x08)));

    // __mul_loop:
    out.push(Instruction::new(NOP, AM::Label("__mul_loop".into())));

    // Shift multiplier right, check carry (current bit)
    out.push(Instruction::new(LSR, AM::ZeroPage(ZP_MUL_OPERAND)));
    out.push(Instruction::new(
        BCC,
        AM::LabelRelative("__mul_no_add".into()),
    ));

    // Carry set: add multiplicand to result
    // Add low byte
    out.push(Instruction::implied(CLC));
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_MUL_RESULT_HI)));
    out.push(Instruction::new(ADC, AM::ZeroPage(0x04)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_MUL_RESULT_HI)));

    // __mul_no_add:
    out.push(Instruction::new(NOP, AM::Label("__mul_no_add".into())));

    // Shift multiplicand left (double it) for next bit position
    out.push(Instruction::new(ASL, AM::ZeroPage(0x04)));

    // Decrement counter
    out.push(Instruction::implied(DEX));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__mul_loop".into()),
    ));

    // Load low byte of result into A
    // For 8-bit result, just use the high byte accumulation
    // (since we shifted the multiplicand left, result is in $03)
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_MUL_RESULT_HI)));

    out.push(Instruction::implied(RTS));

    out
}

/// Generate the per-NMI audio tick that ages the SFX counter and
/// silences pulse 1 when the counter hits zero. Music on pulse 2
/// uses the sentinel `$FF` for infinite sustain and is never
/// decremented here — `stop_music` handles mute explicitly.
///
/// The linker splices a `JSR __audio_tick` into the NMI handler
/// whenever user code contains any audio ops, so programs that
/// never call `play`/`start_music`/`stop_music` pay zero cost.
///
/// Contract:
/// - Input: `ZP_SFX_COUNTER` = remaining frames for pulse 1's tone
/// - Effect: decrements the counter; on 0→transition mutes $4000
/// - Clobbers: A (which the NMI handler restores via PLA)
pub fn gen_audio_tick() -> Vec<Instruction> {
    let mut out = Vec::new();

    out.push(Instruction::new(NOP, AM::Label("__audio_tick".into())));

    // SFX counter check: if 0, nothing to do.
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_SFX_COUNTER)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__audio_tick_done".into()),
    ));
    out.push(Instruction::new(DEC, AM::ZeroPage(ZP_SFX_COUNTER)));
    // If still non-zero, leave the tone alone.
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_tick_done".into()),
    ));
    // Counter just hit 0: silence pulse 1 (volume envelope = mute).
    out.push(Instruction::new(LDA, AM::Immediate(0x30)));
    out.push(Instruction::new(STA, AM::Absolute(0x4000)));

    out.push(Instruction::new(NOP, AM::Label("__audio_tick_done".into())));
    out.push(Instruction::implied(RTS));

    out
}

/// Generate 8 / 8 -> 8 software divide routine (restoring division).
///
/// Input: A = dividend, zero-page $02 = divisor
/// Output: A = quotient, $03 = remainder
pub fn gen_divide() -> Vec<Instruction> {
    let mut out = Vec::new();

    // Label for the subroutine entry
    out.push(Instruction::new(NOP, AM::Label("__divide".into())));

    // Store dividend in $04
    out.push(Instruction::new(STA, AM::ZeroPage(0x04)));

    // Clear remainder
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_DIV_REMAINDER)));

    // Loop counter: 8 bits
    out.push(Instruction::new(LDX, AM::Immediate(0x08)));

    // __div_loop:
    out.push(Instruction::new(NOP, AM::Label("__div_loop".into())));

    // Shift dividend left into remainder
    out.push(Instruction::new(ASL, AM::ZeroPage(0x04)));
    out.push(Instruction::new(ROL, AM::ZeroPage(ZP_DIV_REMAINDER)));

    // Try to subtract divisor from remainder
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_DIV_REMAINDER)));
    out.push(Instruction::implied(SEC));
    out.push(Instruction::new(SBC, AM::ZeroPage(ZP_DIV_DIVISOR)));

    // If remainder >= divisor (no borrow), keep subtraction
    out.push(Instruction::new(
        BCC,
        AM::LabelRelative("__div_no_sub".into()),
    ));

    // Store updated remainder
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_DIV_REMAINDER)));

    // Set bit 0 of quotient (in $04, which we shifted left)
    out.push(Instruction::new(INC, AM::ZeroPage(0x04)));

    // __div_no_sub:
    out.push(Instruction::new(NOP, AM::Label("__div_no_sub".into())));

    // Decrement counter
    out.push(Instruction::implied(DEX));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__div_loop".into()),
    ));

    // Load quotient into A
    out.push(Instruction::new(LDA, AM::ZeroPage(0x04)));

    out.push(Instruction::implied(RTS));

    out
}
