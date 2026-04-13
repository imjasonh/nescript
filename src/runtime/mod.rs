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
/// Pulse-1 SFX envelope pointer (2 bytes, lo/hi) — points at the
/// *current* frame's $4000 envelope byte inside the sfx blob. The
/// audio tick reads through this byte, writes to $4000, advances
/// the pointer, and keeps going until it reads a zero sentinel.
pub const ZP_SFX_PTR_LO: u8 = 0x0C;
pub const ZP_SFX_PTR_HI: u8 = 0x0D;
/// Pulse-2 music note-stream pointer (2 bytes, lo/hi) — points at
/// the *current* (pitch, duration) note pair inside the music blob.
pub const ZP_MUSIC_PTR_LO: u8 = 0x0E;
pub const ZP_MUSIC_PTR_HI: u8 = 0x0F;
/// Music base pointer (2 bytes) — start of the currently-loaded
/// track. Used by the loop-back branch when the driver hits the
/// end-of-track sentinel and the header loop flag is set.
pub const ZP_MUSIC_BASE_LO: u8 = 0x05;
pub const ZP_MUSIC_BASE_HI: u8 = 0x06;
/// Music state byte. Bit layout:
///   bit 0: 1 = track is looping, 0 = one-shot
///   bit 1: 1 = music is active (non-zero means "playing")
///   bits 2-5: latched pulse-2 envelope volume 0-15
///   bits 6-7: latched pulse-2 duty
/// Set on `start_music`, cleared (to 0) on `stop_music`. The driver
/// writes a fresh $4004 envelope byte every time it advances to a
/// new note using these bits so held notes don't decay.
pub const ZP_MUSIC_STATE: u8 = 0x07;
/// Pulse-1 SFX countdown — `0` means no sfx is playing.
/// Nonzero means the audio tick should read one envelope byte from
/// `ZP_SFX_PTR` each NMI and write it to $4000. When the tick reads
/// a zero sentinel it mutes pulse 1 and clears this byte.
pub const ZP_SFX_COUNTER: u8 = 0x0A;
/// Pulse-2 music duration countdown — frames remaining on the
/// currently-held music note. When it reaches zero, the tick
/// advances to the next (pitch, duration) pair.
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

/// Generate the per-NMI audio tick. This is the heart of the audio
/// driver — it walks both the SFX envelope and the music note stream
/// every frame and writes the resulting APU register values.
///
/// The linker splices a `JSR __audio_tick` into the NMI handler
/// whenever user code contains any audio op (detected by the
/// `__audio_used` marker label), so programs that never call
/// `play`/`start_music`/`stop_music` pay zero ROM or cycle cost.
///
/// ## SFX channel (pulse 1)
///
/// State:
/// - `ZP_SFX_COUNTER` — nonzero while an sfx is playing
/// - `ZP_SFX_PTR_LO/HI` — pointer into the current sfx blob,
///   advanced one byte per frame
///
/// Each frame: if the counter is nonzero, read one byte through the
/// pointer, write it to `$4000`, and advance the pointer. A zero
/// byte is the sentinel; on it the driver mutes pulse 1 and clears
/// the counter.
///
/// ## Music channel (pulse 2)
///
/// State:
/// - `ZP_MUSIC_COUNTER` — frames remaining on the current note
/// - `ZP_MUSIC_STATE` — bit 1 set = active; bits encode duty/volume/loop
/// - `ZP_MUSIC_PTR_LO/HI` — pointer to the next (pitch,dur) pair
/// - `ZP_MUSIC_BASE_LO/HI` — loop-back start of the current track
///
/// Each frame: if the state says "active" and the counter is nonzero,
/// decrement the counter and bail. When it hits zero, advance past
/// the current (pitch,dur) pair and read the next one. `0xFF,0xFF`
/// is the end-of-track sentinel; the driver either rewinds to the
/// base pointer (looping tracks) or mutes pulse 2 (one-shot tracks).
///
/// ## Clobbers
///
/// A, X, Y. The NMI handler calls this from inside its own
/// save/restore block so caller registers are safe.
pub fn gen_audio_tick() -> Vec<Instruction> {
    let mut out = Vec::new();

    out.push(Instruction::new(NOP, AM::Label("__audio_tick".into())));

    // ── SFX tick ──
    // If counter is zero, no sfx is playing; skip.
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_SFX_COUNTER)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__audio_sfx_done".into()),
    ));
    // Read next envelope byte via (ZP_SFX_PTR),Y with Y=0.
    out.push(Instruction::new(LDY, AM::Immediate(0)));
    out.push(Instruction::new(LDA, AM::IndirectY(ZP_SFX_PTR_LO)));
    // If it's the zero sentinel, silence pulse 1 and clear state.
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_sfx_write".into()),
    ));
    // Sentinel branch: write mute byte to $4000 and clear counter.
    out.push(Instruction::new(LDA, AM::Immediate(0x30)));
    out.push(Instruction::new(STA, AM::Absolute(0x4000)));
    out.push(Instruction::new(LDA, AM::Immediate(0)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_SFX_COUNTER)));
    out.push(Instruction::new(JMP, AM::Label("__audio_sfx_done".into())));
    // Non-sentinel branch: write envelope byte to $4000, advance ptr.
    out.push(Instruction::new(NOP, AM::Label("__audio_sfx_write".into())));
    out.push(Instruction::new(STA, AM::Absolute(0x4000)));
    // Advance the 16-bit pointer (lo, hi) by 1.
    out.push(Instruction::new(INC, AM::ZeroPage(ZP_SFX_PTR_LO)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_sfx_ptr_ok".into()),
    ));
    out.push(Instruction::new(INC, AM::ZeroPage(ZP_SFX_PTR_HI)));
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_sfx_ptr_ok".into()),
    ));
    out.push(Instruction::new(NOP, AM::Label("__audio_sfx_done".into())));

    // ── Music tick ──
    // Bit 1 of ZP_MUSIC_STATE is "music is active". If clear, skip.
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_MUSIC_STATE)));
    out.push(Instruction::new(AND, AM::Immediate(0x02)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__audio_music_done".into()),
    ));
    // Active. Decrement the note counter; if nonzero after, bail.
    out.push(Instruction::new(DEC, AM::ZeroPage(ZP_MUSIC_COUNTER)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_music_done".into()),
    ));
    // Counter just hit zero — time to advance. Fall through to the
    // "advance to next note" block below. The runtime calls this
    // block from two places: end-of-note and start_music (which sets
    // counter=0 then jumps here to trigger the first note).
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_music_advance".into()),
    ));
    // Read the next pitch byte. LDA sets Z based on the value so
    // we can dispatch on it cheaply:
    //   pitch == 0    → rest     (fall through to __rest)
    //   pitch == 0xFF → sentinel (BNE past rest, then CMP + BEQ)
    //   otherwise     → pitched  (fall through to __pitched)
    out.push(Instruction::new(LDY, AM::Immediate(0)));
    out.push(Instruction::new(LDA, AM::IndirectY(ZP_MUSIC_PTR_LO)));
    // Zero? → rest branch (mute pulse 2, skip period lookup).
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_music_not_rest".into()),
    ));
    out.push(Instruction::new(LDA, AM::Immediate(0x30)));
    out.push(Instruction::new(STA, AM::Absolute(0x4004)));
    out.push(Instruction::new(
        JMP,
        AM::Label("__audio_music_load_dur".into()),
    ));
    // Not zero — check sentinel, otherwise it's a real note.
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_music_not_rest".into()),
    ));
    out.push(Instruction::new(CMP, AM::Immediate(0xFF)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__audio_music_eot".into()),
    ));
    // Fall through to the pitched branch — A still holds pitch.
    out.push(Instruction::new(
        JMP,
        AM::Label("__audio_music_pitched".into()),
    ));
    // Pitched branch: A already holds pitch (1..=60). Index the
    // period table and write $4006 (period lo) and $4007 (period
    // hi + length counter). Each table entry is 2 bytes.
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_music_pitched".into()),
    ));
    // Rewrite envelope byte ($4004) from music state so we don't
    // depend on pulse-2 length counter. Extract duty (bits 6-7) and
    // volume (bits 2-5) from state, shift into position, OR with $30
    // (length-halt + constant volume), write $4004.
    //
    // Save pitch in X so we still have it for the period lookup.
    out.push(Instruction::new(TAX, AM::Implied));
    // Build envelope byte.
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_MUSIC_STATE)));
    out.push(Instruction::new(AND, AM::Immediate(0xC0))); // keep duty bits
    out.push(Instruction::new(STA, AM::ZeroPage(0x04))); // scratch
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_MUSIC_STATE)));
    out.push(Instruction::new(AND, AM::Immediate(0x3C))); // keep volume<<2
    out.push(Instruction::new(LSR, AM::Accumulator));
    out.push(Instruction::new(LSR, AM::Accumulator));
    out.push(Instruction::new(ORA, AM::ZeroPage(0x04)));
    out.push(Instruction::new(ORA, AM::Immediate(0x30)));
    out.push(Instruction::new(STA, AM::Absolute(0x4004)));

    // Period lookup via a ZP pointer. X holds pitch (1..=60).
    //
    //   1. Set (ZP_SCRATCH = __period_table).
    //   2. A = (pitch - 1) * 2 — byte offset in the 2-byte-per-entry
    //      table.
    //   3. Y = A.
    //   4. LDA (ZP_SCRATCH),Y → period_lo → STA $4006.
    //   5. INY; LDA (ZP_SCRATCH),Y → period_hi → STA $4007.
    //
    // `$02`/`$03` are the multiply/divide scratch slots but the NMI
    // audio tick never calls mul/div, so they're free to reuse here.
    // A proper `Absolute,Y` addressing mode with a symbolic label
    // would save the pointer setup, but our asm layer doesn't have
    // that yet and the extra 8 cycles per frame are negligible.
    out.push(Instruction::new(LDA, AM::SymbolLo("__period_table".into())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x02)));
    out.push(Instruction::new(LDA, AM::SymbolHi("__period_table".into())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x03)));
    out.push(Instruction::new(TXA, AM::Implied));
    out.push(Instruction::new(SEC, AM::Implied));
    out.push(Instruction::new(SBC, AM::Immediate(1)));
    out.push(Instruction::new(ASL, AM::Accumulator));
    out.push(Instruction::new(TAY, AM::Implied));
    out.push(Instruction::new(LDA, AM::IndirectY(0x02)));
    out.push(Instruction::new(STA, AM::Absolute(0x4006)));
    out.push(Instruction::new(INY, AM::Implied));
    out.push(Instruction::new(LDA, AM::IndirectY(0x02)));
    // The period-table high byte already has the length-counter
    // load bits baked in (see `gen_period_table`), so a raw store
    // here retriggers the note. But retriggering every time the
    // duration expires is fine — it's how trackers work.
    out.push(Instruction::new(STA, AM::Absolute(0x4007)));

    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_music_load_dur".into()),
    ));
    // Advance pointer past the pitch byte we just consumed.
    out.push(Instruction::new(INC, AM::ZeroPage(ZP_MUSIC_PTR_LO)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_music_dur_hi_ok".into()),
    ));
    out.push(Instruction::new(INC, AM::ZeroPage(ZP_MUSIC_PTR_HI)));
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_music_dur_hi_ok".into()),
    ));
    // Read duration through the advanced pointer and stash it in
    // ZP_MUSIC_COUNTER.
    out.push(Instruction::new(LDY, AM::Immediate(0)));
    out.push(Instruction::new(LDA, AM::IndirectY(ZP_MUSIC_PTR_LO)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_MUSIC_COUNTER)));
    // Advance past the duration byte.
    out.push(Instruction::new(INC, AM::ZeroPage(ZP_MUSIC_PTR_LO)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_music_ptr2_ok".into()),
    ));
    out.push(Instruction::new(INC, AM::ZeroPage(ZP_MUSIC_PTR_HI)));
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_music_ptr2_ok".into()),
    ));
    out.push(Instruction::new(
        JMP,
        AM::Label("__audio_music_done".into()),
    ));

    // ── End-of-track branch ──
    out.push(Instruction::new(NOP, AM::Label("__audio_music_eot".into())));
    // Check loop flag (bit 0 of ZP_MUSIC_STATE). If set, rewind ptr
    // to base and re-enter the advance path. Otherwise stop.
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_MUSIC_STATE)));
    out.push(Instruction::new(AND, AM::Immediate(0x01)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__audio_music_stop".into()),
    ));
    // Looping: copy base pointer back into current pointer and
    // re-enter the advance path.
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_MUSIC_BASE_LO)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_MUSIC_PTR_LO)));
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_MUSIC_BASE_HI)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_MUSIC_PTR_HI)));
    out.push(Instruction::new(
        JMP,
        AM::Label("__audio_music_advance".into()),
    ));
    // Non-looping stop: mute pulse 2 and clear music state.
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_music_stop".into()),
    ));
    out.push(Instruction::new(LDA, AM::Immediate(0x30)));
    out.push(Instruction::new(STA, AM::Absolute(0x4004)));
    out.push(Instruction::new(LDA, AM::Immediate(0)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_MUSIC_STATE)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_MUSIC_COUNTER)));

    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_music_done".into()),
    ));
    out.push(Instruction::implied(RTS));

    out
}

/// Generate the builtin period table that the music tick uses to
/// translate note indices into pulse-channel period values. The
/// table covers five octaves (C1–B5) for 60 entries, 2 bytes each.
///
/// Entry 0 is `C1` (index 1 in user notes), entry 59 is `B5` (index
/// 60). Pitch 0 is the "rest" sentinel and is not present in the
/// table — the driver handles rests before indexing.
///
/// The high byte of each entry is `((period >> 8) & 0x07) | 0x08`.
/// Setting bit 3 pre-loads the length counter to index 1 (254 frames)
/// so any note held beyond the envelope will still play out naturally
/// when the track later falls into a rest — without this, pulse 2
/// would silence itself after ~4 frames on hardware.
#[must_use]
pub fn gen_period_table() -> Vec<Instruction> {
    // NTSC CPU = 1.789773 MHz. Pulse channel frequency:
    //   f = CPU / (16 * (period + 1))
    // Solving for period given a target frequency f:
    //   period = CPU / (16 * f) - 1
    //
    // We compute the 60 entries once at build time (here) using
    // equal-tempered tuning anchored at A4 = 440 Hz.
    const CPU: f64 = 1_789_773.0;
    const A4_HZ: f64 = 440.0;

    let mut out = Vec::new();
    out.push(Instruction::new(NOP, AM::Label("__period_table".into())));
    // Semitone offset from A4 for index `i` (0-based from C1).
    // A4 is MIDI 69. C1 is MIDI 24. So semitones from A4 to C1 is
    // -45 — our table starts at C1 so `offset(i) = i - 45`.
    let mut bytes: Vec<u8> = Vec::with_capacity(120);
    for i in 0i32..60 {
        let semitone_offset = f64::from(i - 45);
        let freq = A4_HZ * 2f64.powf(semitone_offset / 12.0);
        let period_f = CPU / (16.0 * freq) - 1.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let period = period_f.round().clamp(0.0, 2047.0) as u16;
        let lo = (period & 0xFF) as u8;
        // High 3 bits of period + length counter load bits.
        // 0x08 = length counter index 1 = 254 frames.
        let hi = ((period >> 8) as u8 & 0x07) | 0x08;
        bytes.push(lo);
        bytes.push(hi);
    }
    out.push(Instruction::new(NOP, AM::Bytes(bytes)));
    out
}

/// Generate a labelled data block emitting `bytes` verbatim into the
/// ROM at the address the assembler places this block. Used by the
/// linker to splice compiled sfx and music blobs into the code
/// section so that `LDA #<Name; STA ptr_lo` from the IR codegen can
/// resolve to the right in-ROM address.
#[must_use]
pub fn gen_data_block(label: &str, bytes: Vec<u8>) -> Vec<Instruction> {
    vec![
        Instruction::new(NOP, AM::Label(label.to_string())),
        Instruction::new(NOP, AM::Bytes(bytes)),
    ]
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
