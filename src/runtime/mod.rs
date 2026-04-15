#[cfg(test)]
mod tests;

use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};
use crate::parser::ast::{Mapper, Mirroring};

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

// ── PPU update handshake ──
//
// When a program declares `palette` or `background` blocks the
// analyzer reserves `$11-$17` as runtime state for the vblank-safe
// update path. User code sets these from inside a frame handler
// (via `set_palette` / `load_background`), and the NMI handler
// applies any pending update while the PPU is blanked, then
// clears the flags.

/// Bitfield of pending PPU updates.
///   bit 0 = 1 → palette at `ZP_PENDING_PALETTE_*` is pending
///   bit 1 = 1 → background at `ZP_PENDING_BG_TILES_*` / `_ATTRS_*` is pending
pub const ZP_PPU_UPDATE_FLAGS: u8 = 0x11;
pub const ZP_PENDING_PALETTE_LO: u8 = 0x12;
pub const ZP_PENDING_PALETTE_HI: u8 = 0x13;
pub const ZP_PENDING_BG_TILES_LO: u8 = 0x14;
pub const ZP_PENDING_BG_TILES_HI: u8 = 0x15;
pub const ZP_PENDING_BG_ATTRS_LO: u8 = 0x16;
pub const ZP_PENDING_BG_ATTRS_HI: u8 = 0x17;

// ── Debug instrumentation ──
//
// These slots are only touched by debug-mode ROMs. In release
// builds the analyzer is free to allocate over them.

/// Debug-mode frame-overrun counter. Incremented by the NMI
/// handler whenever it fires while the previous frame's ready
/// flag is still set — which means the main loop didn't consume
/// it, so user code spent more than one vblank-to-vblank window
/// processing the last frame. Read it with `peek(0x07FF)` or
/// `debug.frame_overrun_count()` in user code to see how many
/// overruns have happened since reset, or watch the address in
/// a Mesen memory viewer. Placed at the top of main RAM to
/// minimise the chance of a collision with analyzer-allocated
/// variables (which grow from $0300 upward).
pub const DEBUG_FRAME_OVERRUN_ADDR: u16 = 0x07FF;

/// Debug-mode "did the previous frame overrun" sticky bit. Set
/// to 1 by the NMI handler at the same time as it bumps
/// [`DEBUG_FRAME_OVERRUN_ADDR`], and cleared to 0 by `wait_frame`
/// once the main loop catches up. Exposed to user code as
/// `debug.frame_overran()` — a per-frame "did this frame finish
/// in time" predicate suited for `debug.assert(!debug.frame_overran())`
/// guards. Lives one byte below the cumulative counter so the
/// two can be inspected together in a Mesen memory viewer.
pub const DEBUG_FRAME_OVERRUN_FLAG_ADDR: u16 = 0x07FE;

// ── Extra channel state ──
//
// The pulse-1 sfx and pulse-2 music channels live in zero page
// ($00-$0F) where every byte is precious. Adding new channel
// state there would either push user variables back by 6 bytes
// (breaking every existing example's ZP layout) or collide with
// runtime scratch slots. Instead, we park triangle and noise
// state at the very top of main RAM, just below the debug frame
// overrun counter, where analyzer-allocated globals rarely reach
// (they grow from $0300 upward). The few extra cycles per
// absolute access are negligible for a once-per-NMI tick.
//
// The state is only *referenced* by the audio tick when the
// corresponding `has_noise` / `has_triangle` flag is set — so
// programs that don't declare any noise/triangle sfx touch
// these addresses zero times, and the ROM bytes generated for
// an existing audio example are byte-identical to what today's
// compiler produces.
pub const AUDIO_NOISE_PTR_LO: u16 = 0x07F0;
pub const AUDIO_NOISE_PTR_HI: u16 = 0x07F1;
pub const AUDIO_NOISE_COUNTER: u16 = 0x07F2;
pub const AUDIO_TRIANGLE_PTR_LO: u16 = 0x07F3;
pub const AUDIO_TRIANGLE_PTR_HI: u16 = 0x07F4;
pub const AUDIO_TRIANGLE_COUNTER: u16 = 0x07F5;

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

    // Enable NMI so the frame handshake fires every vblank. We
    // deliberately leave PPU_MASK at 0 (rendering fully disabled)
    // here — the linker splices in palette and background loads
    // after this init, and $2007 writes during active rendering
    // corrupt their target addresses via the PPU's v-register
    // auto-increment glitch. Rendering is enabled by the linker
    // *after* all initial VRAM loads complete, via `gen_enable_rendering`.
    out.push(Instruction::new(LDA, AM::Immediate(0x80))); // enable NMI
    out.push(Instruction::new(STA, AM::Absolute(PPU_CTRL)));

    out
}

/// Emit the `PPU_MASK` write that turns on rendering. Called by
/// the linker at the very end of the reset path, after all
/// initial palette / background loads are done, so the initial
/// VRAM writes are never corrupted by a mid-frame `$2007` glitch.
///
/// `show_background` controls whether the background layer is
/// enabled alongside the sprite layer — programs that declare a
/// `background` block want both, programs that don't can skip
/// the background bit to match the pre-fix behaviour.
#[must_use]
pub fn gen_enable_rendering(show_background: bool) -> Vec<Instruction> {
    // $1E = show bg + sprites + left-8-px for both
    // $10 = show sprites only (no bg)
    let mask = if show_background { 0x1E } else { 0x10 };
    vec![
        Instruction::new(LDA, AM::Immediate(mask)),
        Instruction::new(STA, AM::Absolute(PPU_MASK)),
    ]
}

/// Generate the NMI handler.
/// Called every vblank by the NES hardware.
///
/// `has_ppu_updates` controls whether the handler runs the
/// palette / nametable update helper. When false, the handler skips
/// that block entirely so programs that never call `set_palette` /
/// `load_background` pay zero cycles or bytes for the feature.
///
/// `has_audio` controls whether the handler calls the audio tick.
/// When true, the JSR to `__audio_tick` is emitted *after* the
/// register and scratch-slot saves, so the tick is free to trash
/// A/X/Y and the mul/state ZP scratch ($02/$03) without corrupting
/// the user's main-loop state. Placing the JSR outside the
/// save/restore window used to silently clobber `ZP_CURRENT_STATE`
/// whenever a music note was played (the tick's period-table
/// lookup stashes the table's high byte into $03).
///
/// `debug_mode` enables frame-overrun detection: before touching
/// the frame-ready flag, the handler checks whether it's already
/// set — if it is, the previous frame's main-loop work never
/// finished (i.e. the program ran over its vblank budget) and
/// the handler bumps the counter at
/// [`DEBUG_FRAME_OVERRUN_ADDR`]. Release-mode ROMs never call
/// this with `debug_mode=true`, so the counter slot stays free
/// for user allocation.
#[must_use]
pub fn gen_nmi(has_ppu_updates: bool, has_audio: bool, debug_mode: bool) -> Vec<Instruction> {
    let mut out = Vec::new();

    // Save registers
    out.push(Instruction::implied(PHA));
    out.push(Instruction::implied(TXA));
    out.push(Instruction::implied(PHA));
    out.push(Instruction::implied(TYA));
    out.push(Instruction::implied(PHA));

    // Save the multiply/divide scratch slots ($02/$03). $03 doubles
    // as `ZP_CURRENT_STATE` for the state dispatch, and user code
    // mid-multiply/divide has both slots live; preserving them here
    // keeps the invariant that NMI never clobbers user-visible ZP
    // state.
    out.push(Instruction::new(LDA, AM::ZeroPage(0x02)));
    out.push(Instruction::implied(PHA));
    out.push(Instruction::new(LDA, AM::ZeroPage(0x03)));
    out.push(Instruction::implied(PHA));

    // Run the audio driver's per-frame tick *after* the saves so it
    // can freely reuse A/X/Y and the $02/$03 scratch slots without
    // corrupting anything the main loop cares about. Programs that
    // never touch audio skip this splice entirely — no ROM cost.
    if has_audio {
        out.push(Instruction::new(JSR, AM::Label("__audio_tick".into())));
    }

    // OAM DMA — transfer sprite data from $0200
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::Absolute(OAM_ADDR)));
    out.push(Instruction::new(LDA, AM::Immediate(0x02)));
    out.push(Instruction::new(STA, AM::Absolute(OAM_DMA)));

    // PPU updates: check the flags byte, apply any pending palette
    // or background write. Runs inside vblank where $2006/$2007
    // writes are safe. Gated on `has_ppu_updates` so programs that
    // never touch palette or background decls skip this entirely.
    if has_ppu_updates {
        out.extend(gen_ppu_update_apply());
    }

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

    // Debug frame-overrun check. The frame flag is "set on NMI,
    // cleared by wait_frame". If we see it set at the top of a
    // new NMI, the main loop never reached its wait_frame since
    // the previous vblank — i.e. the frame overran. Bump a
    // counter at `DEBUG_FRAME_OVERRUN_ADDR` in that case so user
    // code can `peek(0x07FF)` to see how many overruns have
    // happened. The check is gated on `debug_mode` so release
    // builds emit nothing here.
    if debug_mode {
        // Read the previous flag. If zero, skip the bump.
        out.push(Instruction::new(LDA, AM::ZeroPage(ZP_FRAME_FLAG)));
        out.push(Instruction::new(
            BEQ,
            AM::LabelRelative("__debug_no_overrun".into()),
        ));
        out.push(Instruction::new(
            INC,
            AM::Absolute(DEBUG_FRAME_OVERRUN_ADDR),
        ));
        // Set the per-frame sticky bit. It stays set until the
        // next `wait_frame` clears it, so a single
        // `debug.assert(!debug.frame_overran())` guard at the top
        // of `on frame { ... }` catches any miss in the previous
        // window.
        out.push(Instruction::new(LDA, AM::Immediate(0x01)));
        out.push(Instruction::new(
            STA,
            AM::Absolute(DEBUG_FRAME_OVERRUN_FLAG_ADDR),
        ));
        out.push(Instruction::new(
            NOP,
            AM::Label("__debug_no_overrun".into()),
        ));
    }

    // Set frame-ready flag
    out.push(Instruction::new(LDA, AM::Immediate(0x01)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_FRAME_FLAG)));

    // Restore the mul/state scratch slots ($03 then $02, reverse
    // order of the PHA pushes above).
    out.push(Instruction::implied(PLA));
    out.push(Instruction::new(STA, AM::ZeroPage(0x03)));
    out.push(Instruction::implied(PLA));
    out.push(Instruction::new(STA, AM::ZeroPage(0x02)));

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

/// Generate the in-NMI PPU update helper. Checks
/// [`ZP_PPU_UPDATE_FLAGS`] and, if any bit is set, copies the
/// corresponding blob from PRG ROM to PPU RAM via `$2006`/`$2007`.
/// Safe because the NMI fires at the start of vblank, giving
/// ~2273 CPU cycles of safe PPU write time — enough for a full
/// palette (32 bytes, ~200 cycles) plus a full nametable
/// (1024 bytes, ~6500 cycles; this doesn't fit in a single frame
/// so big updates should be staged by the caller).
///
/// For simplicity and to keep the NMI bounded, this helper writes
/// the palette first and the nametable second, and only one of
/// each can be pending at a time. If a nametable write is larger
/// than vblank allows the program is responsible for either
/// keeping rendering disabled or splitting the update.
///
/// The helper clears the pending flag only for updates it actually
/// applied, so if a program ever queues a palette and a nametable
/// in the same frame both land on the same NMI.
fn gen_ppu_update_apply() -> Vec<Instruction> {
    let mut out = Vec::new();

    // Read flags. If zero, jump straight to the done label.
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_PPU_UPDATE_FLAGS)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__ppu_update_done".into()),
    ));

    // ── palette update (bit 0) ────────────────────────────────
    // Check bit 0; if clear, skip to background.
    out.push(Instruction::new(AND, AM::Immediate(0x01)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__ppu_update_no_palette".into()),
    ));
    // Set PPU addr to $3F00.
    out.push(Instruction::new(LDA, AM::Absolute(PPU_STATUS)));
    out.push(Instruction::new(LDA, AM::Immediate(0x3F)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));
    // Loop: write 32 bytes via `LDA (zp),Y` indirect-indexed from
    // the pending palette pointer at $12/$13.
    out.push(Instruction::new(LDY, AM::Immediate(0x00)));
    out.push(Instruction::new(NOP, AM::Label("__ppu_pal_loop".into())));
    out.push(Instruction::new(LDA, AM::IndirectY(ZP_PENDING_PALETTE_LO)));
    out.push(Instruction::new(STA, AM::Absolute(0x2007)));
    out.push(Instruction::implied(INY));
    out.push(Instruction::new(CPY, AM::Immediate(32)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__ppu_pal_loop".into()),
    ));

    out.push(Instruction::new(
        NOP,
        AM::Label("__ppu_update_no_palette".into()),
    ));

    // ── background update (bit 1) ────────────────────────────
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_PPU_UPDATE_FLAGS)));
    out.push(Instruction::new(AND, AM::Immediate(0x02)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__ppu_update_no_bg".into()),
    ));
    // Nametable 0 starts at $2000.
    out.push(Instruction::new(LDA, AM::Absolute(PPU_STATUS)));
    out.push(Instruction::new(LDA, AM::Immediate(0x20)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));
    // Write 960 bytes as 4 loops of 240 (so Y fits in u8) — simpler
    // to write as an outer X counter across 4 × 240-byte pages.
    // X = 4 pages to go
    out.push(Instruction::new(LDX, AM::Immediate(4)));
    out.push(Instruction::new(NOP, AM::Label("__ppu_bg_outer".into())));
    out.push(Instruction::new(LDY, AM::Immediate(0x00)));
    out.push(Instruction::new(NOP, AM::Label("__ppu_bg_inner".into())));
    out.push(Instruction::new(LDA, AM::IndirectY(ZP_PENDING_BG_TILES_LO)));
    out.push(Instruction::new(STA, AM::Absolute(0x2007)));
    out.push(Instruction::implied(INY));
    out.push(Instruction::new(CPY, AM::Immediate(240)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__ppu_bg_inner".into()),
    ));
    // After each 240-byte block, bump the pointer by 240 so the
    // next block reads from the following chunk of the blob.
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_PENDING_BG_TILES_LO)));
    out.push(Instruction::new(CLC, AM::Implied));
    out.push(Instruction::new(ADC, AM::Immediate(240)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_PENDING_BG_TILES_LO)));
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_PENDING_BG_TILES_HI)));
    out.push(Instruction::new(ADC, AM::Immediate(0)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_PENDING_BG_TILES_HI)));
    out.push(Instruction::implied(DEX));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__ppu_bg_outer".into()),
    ));
    // Now write the 64-byte attribute table (at $23C0 — right after
    // the nametable we just wrote). The PPU auto-increment sits at
    // $23C0 already since we wrote exactly 960 bytes after $2000.
    out.push(Instruction::new(LDY, AM::Immediate(0x00)));
    out.push(Instruction::new(
        NOP,
        AM::Label("__ppu_bg_attr_loop".into()),
    ));
    out.push(Instruction::new(LDA, AM::IndirectY(ZP_PENDING_BG_ATTRS_LO)));
    out.push(Instruction::new(STA, AM::Absolute(0x2007)));
    out.push(Instruction::implied(INY));
    out.push(Instruction::new(CPY, AM::Immediate(64)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__ppu_bg_attr_loop".into()),
    ));
    out.push(Instruction::new(
        NOP,
        AM::Label("__ppu_update_no_bg".into()),
    ));

    // Clear all pending flags. Programs re-queue every frame if
    // they want repeating updates.
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_PPU_UPDATE_FLAGS)));

    out.push(Instruction::new(NOP, AM::Label("__ppu_update_done".into())));

    out
}

/// Emit a reset-time write of a 32-byte palette blob (referenced
/// by label) to PPU `$3F00-$3F1F`. Rendering must be disabled
/// when this runs (it is, between `gen_init` and the linker's PPU
/// rendering-enable step). Uses the scratch ZP slots `$02/$03` to
/// hold the indirect pointer — safe because nothing else runs
/// between `gen_init` and user code.
#[must_use]
pub fn gen_initial_palette_load(label: &str) -> Vec<Instruction> {
    let mut out = Vec::new();
    out.push(Instruction::new(LDA, AM::Absolute(PPU_STATUS))); // reset latch
    out.push(Instruction::new(LDA, AM::Immediate(0x3F)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));
    // Stash the palette label into scratch ZP for indirect LDA.
    out.push(Instruction::new(LDA, AM::SymbolLo(label.to_string())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x02)));
    out.push(Instruction::new(LDA, AM::SymbolHi(label.to_string())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x03)));
    out.push(Instruction::new(LDY, AM::Immediate(0x00)));
    let loop_label = format!("__init_pal_loop_{label}");
    out.push(Instruction::new(NOP, AM::Label(loop_label.clone())));
    out.push(Instruction::new(LDA, AM::IndirectY(0x02)));
    out.push(Instruction::new(STA, AM::Absolute(0x2007)));
    out.push(Instruction::implied(INY));
    out.push(Instruction::new(CPY, AM::Immediate(32)));
    out.push(Instruction::new(BNE, AM::LabelRelative(loop_label)));
    out
}

/// Emit a reset-time write of a 960-byte nametable + 64-byte
/// attribute table blob to nametable 0 (`$2000-$23FF`). Rendering
/// must be disabled when this runs. The caller passes the label of
/// the tiles blob and the label of the attribute blob separately —
/// the linker emits them as adjacent data blocks so they can be
/// resolved independently.
#[must_use]
pub fn gen_initial_background_load(tiles_label: &str, attrs_label: &str) -> Vec<Instruction> {
    let mut out = Vec::new();
    out.push(Instruction::new(LDA, AM::Absolute(PPU_STATUS)));
    out.push(Instruction::new(LDA, AM::Immediate(0x20)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));

    // Write 960 bytes of tile data as 4 × 240 using two nested
    // counters. We stash the outer page index in a scratch ZP slot
    // because X is too small to index a 960-byte range directly.
    out.push(Instruction::new(LDX, AM::Immediate(0x00)));
    let page_loop = format!("__init_bg_page_{tiles_label}");
    let inner_loop = format!("__init_bg_inner_{tiles_label}");
    out.push(Instruction::new(NOP, AM::Label(page_loop.clone())));
    // Per-page offset: X*240. Computed via Y and clamped at 240.
    out.push(Instruction::new(LDY, AM::Immediate(0x00)));
    out.push(Instruction::new(NOP, AM::Label(inner_loop.clone())));
    // Fetch byte at blob[X*240 + Y]. We materialize the effective
    // absolute address by unrolling 4 separate LDA Absolute,Y
    // instructions, one per page, dispatched on X.
    // For simplicity and correctness we take the slower path:
    // compute (blob + X*240) as a ZP pointer and read via
    // `LDA (zp),Y`.
    // ZP scratch at $02/$03 (same slots used by the multiply/divide
    // contract; gen_init runs before any user code so they're free).
    out.push(Instruction::new(LDA, AM::SymbolLo(tiles_label.to_string())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x02)));
    out.push(Instruction::new(LDA, AM::SymbolHi(tiles_label.to_string())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x03)));
    // Add X*240 to the low byte (high byte carries via ADC).
    // Actually — to keep this simple, we instead track bytes
    // remaining as a 16-bit counter and use a generic LDA (ZP),Y
    // loop. Rewrite the routine as a flat byte-counted loop.
    // (Undo the per-page setup above by rebuilding the output
    // vector from scratch.)
    out.clear();

    out.push(Instruction::new(LDA, AM::Absolute(PPU_STATUS)));
    out.push(Instruction::new(LDA, AM::Immediate(0x20)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));
    out.push(Instruction::new(LDA, AM::Immediate(0x00)));
    out.push(Instruction::new(STA, AM::Absolute(0x2006)));

    // Load tile blob pointer into $02/$03 scratch slots.
    out.push(Instruction::new(LDA, AM::SymbolLo(tiles_label.to_string())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x02)));
    out.push(Instruction::new(LDA, AM::SymbolHi(tiles_label.to_string())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x03)));

    // 4 pages × 240 bytes each = 960 bytes total.
    out.push(Instruction::new(LDX, AM::Immediate(4)));
    let outer = format!("__init_bg_outer_{tiles_label}");
    let inner = format!("__init_bg_inner_{tiles_label}");
    out.push(Instruction::new(NOP, AM::Label(outer.clone())));
    out.push(Instruction::new(LDY, AM::Immediate(0x00)));
    out.push(Instruction::new(NOP, AM::Label(inner.clone())));
    out.push(Instruction::new(LDA, AM::IndirectY(0x02)));
    out.push(Instruction::new(STA, AM::Absolute(0x2007)));
    out.push(Instruction::implied(INY));
    out.push(Instruction::new(CPY, AM::Immediate(240)));
    out.push(Instruction::new(BNE, AM::LabelRelative(inner)));
    // Advance pointer by 240.
    out.push(Instruction::new(LDA, AM::ZeroPage(0x02)));
    out.push(Instruction::new(CLC, AM::Implied));
    out.push(Instruction::new(ADC, AM::Immediate(240)));
    out.push(Instruction::new(STA, AM::ZeroPage(0x02)));
    out.push(Instruction::new(LDA, AM::ZeroPage(0x03)));
    out.push(Instruction::new(ADC, AM::Immediate(0)));
    out.push(Instruction::new(STA, AM::ZeroPage(0x03)));
    out.push(Instruction::implied(DEX));
    out.push(Instruction::new(BNE, AM::LabelRelative(outer)));

    // Now the 64 attribute bytes land at $23C0 — the PPU auto-
    // increment is already there after the 960 tile writes.
    out.push(Instruction::new(LDA, AM::SymbolLo(attrs_label.to_string())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x02)));
    out.push(Instruction::new(LDA, AM::SymbolHi(attrs_label.to_string())));
    out.push(Instruction::new(STA, AM::ZeroPage(0x03)));
    out.push(Instruction::new(LDY, AM::Immediate(0x00)));
    let attr_loop = format!("__init_bg_attr_{attrs_label}");
    out.push(Instruction::new(NOP, AM::Label(attr_loop.clone())));
    out.push(Instruction::new(LDA, AM::IndirectY(0x02)));
    out.push(Instruction::new(STA, AM::Absolute(0x2007)));
    out.push(Instruction::implied(INY));
    out.push(Instruction::new(CPY, AM::Immediate(64)));
    out.push(Instruction::new(BNE, AM::LabelRelative(attr_loop)));
    out
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
///
/// When `has_noise` / `has_triangle` are set, the driver gains an
/// extra per-channel slot: noise routes envelope bytes to `$400C`
/// and drives `$400E` / `$400F` on trigger; triangle writes linear-
/// counter reload values to `$4008`. These blocks are appended to
/// the tick after the music path so that programs which do not
/// declare any noise or triangle sfx produce byte-identical ROM
/// output — the old pulse-only path emits exactly the same
/// instruction stream as before. The linker decides whether to
/// enable each by scanning for the `__noise_used` and
/// `__triangle_used` marker labels emitted by the IR codegen.
pub fn gen_audio_tick(has_noise: bool, has_triangle: bool) -> Vec<Instruction> {
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

    // ── Noise channel tick (optional, gated on `has_noise`) ──
    //
    // Structurally identical to the pulse-1 sfx tick above: walk a
    // per-frame envelope blob via an indirect-indexed load and
    // write each byte to the APU noise volume register at $400C.
    // The pointer lives in main RAM at [AUDIO_NOISE_PTR_LO,
    // AUDIO_NOISE_PTR_HI]; the tick stashes it in ZP scratch $02/$03
    // for the duration of the block because the 6502 has no
    // (abs),Y addressing.
    if has_noise {
        out.extend(gen_noise_tick());
    }

    // ── Triangle channel tick (optional, gated on `has_triangle`) ──
    //
    // Same shape as the noise tick but writes to $4008 (the linear
    // counter) instead of a volume register. Triangle has no volume,
    // so the envelope blob just encodes "keep the linear counter
    // loaded" (nonzero hold) or "silence" (the $80 sentinel).
    if has_triangle {
        out.extend(gen_triangle_tick());
    }

    out.push(Instruction::implied(RTS));

    out
}

/// Generate the noise-channel sfx tick. Appended to
/// [`gen_audio_tick`] only when the program declares at least one
/// noise sfx (`__noise_used` marker). Reads envelope bytes from the
/// main-RAM pointer at [`AUDIO_NOISE_PTR_LO`] / [`AUDIO_NOISE_PTR_HI`]
/// and writes them to the APU noise volume register at `$400C`.
/// A zero envelope byte is the mute sentinel — on it the tick
/// silences the channel and clears [`AUDIO_NOISE_COUNTER`].
fn gen_noise_tick() -> Vec<Instruction> {
    let mut out = Vec::new();
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_noise_tick".into()),
    ));
    // If counter is zero, no noise sfx is playing; skip the block.
    out.push(Instruction::new(LDA, AM::Absolute(AUDIO_NOISE_COUNTER)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__audio_noise_done".into()),
    ));
    // Load the main-RAM pointer into ZP scratch $02/$03 so we can
    // do an indirect-indexed read (6502 has no (abs),Y mode).
    out.push(Instruction::new(LDA, AM::Absolute(AUDIO_NOISE_PTR_LO)));
    out.push(Instruction::new(STA, AM::ZeroPage(0x02)));
    out.push(Instruction::new(LDA, AM::Absolute(AUDIO_NOISE_PTR_HI)));
    out.push(Instruction::new(STA, AM::ZeroPage(0x03)));
    // Read envelope byte through the scratch pointer.
    out.push(Instruction::new(LDY, AM::Immediate(0)));
    out.push(Instruction::new(LDA, AM::IndirectY(0x02)));
    // Zero sentinel? branch to the write path if nonzero.
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_noise_write".into()),
    ));
    // Sentinel: mute pulse-noise ($4000-compatible encoding:
    // length-halt + constant-volume + volume 0) and clear the
    // counter so this block bails on subsequent NMIs.
    out.push(Instruction::new(LDA, AM::Immediate(0x30)));
    out.push(Instruction::new(STA, AM::Absolute(0x400C)));
    out.push(Instruction::new(LDA, AM::Immediate(0)));
    out.push(Instruction::new(STA, AM::Absolute(AUDIO_NOISE_COUNTER)));
    out.push(Instruction::new(
        JMP,
        AM::Label("__audio_noise_done".into()),
    ));
    // Write envelope byte and advance the 16-bit pointer by 1.
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_noise_write".into()),
    ));
    out.push(Instruction::new(STA, AM::Absolute(0x400C)));
    out.push(Instruction::new(INC, AM::Absolute(AUDIO_NOISE_PTR_LO)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_noise_ptr_ok".into()),
    ));
    out.push(Instruction::new(INC, AM::Absolute(AUDIO_NOISE_PTR_HI)));
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_noise_ptr_ok".into()),
    ));
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_noise_done".into()),
    ));
    out
}

/// Generate the triangle-channel sfx tick. Same shape as
/// [`gen_noise_tick`] but writes to `$4008` (the linear counter
/// reload) instead of a volume register. Triangle has no volume —
/// the envelope bytes are "keep holding" tokens that the runtime
/// keeps writing every frame so the linear counter never underruns
/// and the channel never auto-silences. A `0x80` byte is the mute
/// sentinel (linear control bit set, reload = 0 → silence next
/// frame).
fn gen_triangle_tick() -> Vec<Instruction> {
    let mut out = Vec::new();
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_triangle_tick".into()),
    ));
    // If counter is zero, no triangle sfx is playing.
    out.push(Instruction::new(LDA, AM::Absolute(AUDIO_TRIANGLE_COUNTER)));
    out.push(Instruction::new(
        BEQ,
        AM::LabelRelative("__audio_triangle_done".into()),
    ));
    // Stash pointer to ZP scratch for indirect-indexed load.
    out.push(Instruction::new(LDA, AM::Absolute(AUDIO_TRIANGLE_PTR_LO)));
    out.push(Instruction::new(STA, AM::ZeroPage(0x02)));
    out.push(Instruction::new(LDA, AM::Absolute(AUDIO_TRIANGLE_PTR_HI)));
    out.push(Instruction::new(STA, AM::ZeroPage(0x03)));
    out.push(Instruction::new(LDY, AM::Immediate(0)));
    out.push(Instruction::new(LDA, AM::IndirectY(0x02)));
    // Write the envelope byte to $4008. For triangle, `$80` is the
    // mute sentinel (linear counter reload = 0 with control bit
    // set). We detect it by CMP + BEQ so the counter can be cleared.
    out.push(Instruction::new(STA, AM::Absolute(0x4008)));
    out.push(Instruction::new(CMP, AM::Immediate(0x80)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_triangle_advance".into()),
    ));
    // Sentinel: clear counter so we bail next frame. The $80 write
    // above already mutes the triangle channel.
    out.push(Instruction::new(LDA, AM::Immediate(0)));
    out.push(Instruction::new(STA, AM::Absolute(AUDIO_TRIANGLE_COUNTER)));
    out.push(Instruction::new(
        JMP,
        AM::Label("__audio_triangle_done".into()),
    ));
    // Advance the pointer by 1 for the next frame.
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_triangle_advance".into()),
    ));
    out.push(Instruction::new(INC, AM::Absolute(AUDIO_TRIANGLE_PTR_LO)));
    out.push(Instruction::new(
        BNE,
        AM::LabelRelative("__audio_triangle_ptr_ok".into()),
    ));
    out.push(Instruction::new(INC, AM::Absolute(AUDIO_TRIANGLE_PTR_HI)));
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_triangle_ptr_ok".into()),
    ));
    out.push(Instruction::new(
        NOP,
        AM::Label("__audio_triangle_done".into()),
    ));
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

// ─── Bank switching ────────────────────────────────────────────────
//
// NEScript supports bank switching for MMC1, UxROM, and MMC3. The
// linker lays out PRG ROM with a single fixed bank ($C000-$FFFF)
// holding the runtime, NMI, IRQ vectors, and any cross-bank
// trampolines, plus zero or more switchable 16 KB banks mapped at
// $8000-$BFFF. The helpers below emit:
//
//   * `gen_mapper_init` — reset-time configuration that puts the
//     last physical bank at $C000 and (depending on the mapper)
//     sets a known mirroring mode so the compiler's
//     `Mirroring::{Horizontal,Vertical}` selection matches.
//   * `gen_bank_select` — a subroutine callable with the target bank
//     number in A that selects the correct switchable bank at $8000.
//   * `gen_bank_trampoline` — a per-(target, bank) stub placed in
//     the fixed bank. Callers `JSR` into the trampoline, which
//     records the current bank, switches to the target bank, calls
//     the entry label in that bank, and switches back.
//
// The trampolines never physically return to the switchable bank —
// control is always handed back to the fixed bank after the callee
// returns. Users don't touch these routines directly; the linker
// emits them from the `bank` declarations in the program AST.

/// Zero-page slot used by the bank-select routine to stash the
/// requested bank number so `__bank_return` can restore it when a
/// trampoline finishes.
pub const ZP_BANK_CURRENT: u8 = 0x10;

/// Generate the reset-time mapper initialization sequence. Splices
/// after `gen_init` but before any user code runs. For NROM this is
/// a no-op — `gen_init` already sets up everything NROM needs.
///
/// `total_prg_banks` is the total number of 16 KB PRG banks in the
/// ROM; MMC1/MMC3 need this to fix the *last* physical bank at
/// $C000. `UxROM` is hardwired — its last bank is always fixed.
#[must_use]
pub fn gen_mapper_init(
    mapper: Mapper,
    mirroring: Mirroring,
    total_prg_banks: usize,
) -> Vec<Instruction> {
    let mut out = match mapper {
        Mapper::NROM => Vec::new(),
        Mapper::MMC1 => gen_mmc1_init(mirroring),
        Mapper::UxROM => gen_uxrom_init(total_prg_banks),
        Mapper::MMC3 => gen_mmc3_init(mirroring),
    };
    // Initialize ZP_BANK_CURRENT to the fixed bank index for any
    // banked mapper. The trampoline emitted by
    // `gen_bank_trampoline` reads this slot to decide which bank
    // to restore after a cross-bank call, so it has to be a
    // sensible value from the very first call. Without this the
    // RAM-clear leaves it at $00, which would put bank 0 at
    // $8000 instead of the fixed bank after a fixed-bank caller's
    // first cross-bank call — a behavior change vs. the pre-
    // banked-banked codegen that some examples rely on.
    if mapper != Mapper::NROM && total_prg_banks > 0 {
        #[allow(clippy::cast_possible_truncation)]
        let fixed_bank_index = (total_prg_banks - 1) as u8;
        out.push(Instruction::new(LDA, AM::Immediate(fixed_bank_index)));
        out.push(Instruction::new(STA, AM::ZeroPage(ZP_BANK_CURRENT)));
    }
    out
}

/// MMC1 reset: pulse the reset bit, then write the control register.
/// Control-register layout (5 bits, serialized LSB-first into any
/// $8000-$FFFF address):
///   bit 4   — CHR bank mode (0 = 8 KB, 1 = two 4 KB banks)
///   bit 3   — PRG bank mode bit 1
///   bit 2   — PRG bank mode bit 0
///             11 = 16 KB banks, fix last at $C000, switchable at $8000
///   bit 1-0 — mirroring
///             00 = 1-screen lo, 01 = 1-screen hi,
///             10 = vertical,    11 = horizontal
///
/// We pick mode `11` (fixed last bank) so the fixed bank appears at
/// $C000 exactly the same way as NROM, which lets us reuse the NROM
/// layout for all the runtime code that already exists.
fn gen_mmc1_init(mirroring: Mirroring) -> Vec<Instruction> {
    let mut out = Vec::new();
    out.push(Instruction::new(NOP, AM::Label("__mmc1_init".into())));
    // Reset pulse: any $8000-range write with bit 7 set flushes the
    // 5-bit shift register and resets the internal config to
    // mode 3 (fixed last, 16 KB banks).
    out.push(Instruction::new(LDA, AM::Immediate(0x80)));
    out.push(Instruction::new(STA, AM::Absolute(0x8000)));
    // Control register value.
    let mirror_bits = match mirroring {
        Mirroring::Horizontal => 0b11,
        Mirroring::Vertical => 0b10,
    };
    // 16 KB PRG, fix last at $C000 (bits 2-3 = 11), 8 KB CHR
    // (bit 4 = 0), plus mirroring bits.
    let control: u8 = 0b0_11_00 | mirror_bits;
    // Serialize the 5 bits into the shift register. Each write
    // uses STA $8000 (which maps to the control register because
    // the address falls in the $8000-$9FFF range).
    out.extend(gen_mmc1_serial_write(control, 0x8000));
    out
}

/// Emit 5 serialized writes of `value` to `addr`, shifting right
/// between writes. Used by MMC1 bank-switch code (registers all live
/// in $8000-$FFFF and are selected by the top two address bits).
fn gen_mmc1_serial_write(value: u8, addr: u16) -> Vec<Instruction> {
    let mut out = Vec::new();
    // Bit 0 goes out first, then bit 1, etc. We use immediate loads
    // for each bit so the sequence has no hidden dependencies on
    // the current A register.
    for i in 0..5 {
        let bit = (value >> i) & 1;
        out.push(Instruction::new(LDA, AM::Immediate(bit)));
        out.push(Instruction::new(STA, AM::Absolute(addr)));
    }
    out
}

/// `UxROM` reset: the last 16 KB PRG bank is always fixed at $C000,
/// and the switchable bank at $8000 defaults to bank 0 on power-on.
/// Some `UxROM` boards have bus conflicts — any write must match the
/// byte in ROM — so we use a small bank-select table at a known
/// address (`__bank_select_table`) generated into the fixed bank.
fn gen_uxrom_init(_total_banks: usize) -> Vec<Instruction> {
    // No explicit init required: UxROM powers up with bank 0 at
    // $8000 and the last bank fixed at $C000, which is exactly
    // what we want. We still emit the label so debuggers can find
    // the (empty) init span.
    vec![Instruction::new(NOP, AM::Label("__uxrom_init".into()))]
}

/// MMC3 reset: choose PRG mode 0 (last two banks fixed at
/// $C000-$FFFF) and initialise bank 0 at $8000, bank 1 at $A000.
/// Mirroring is programmed via $A000 (only meaningful when CHR
/// uses the internal mode — for our CHR ROM layout it's still
/// the safest place to latch).
fn gen_mmc3_init(mirroring: Mirroring) -> Vec<Instruction> {
    let mut out = Vec::new();
    out.push(Instruction::new(NOP, AM::Label("__mmc3_init".into())));

    // Select PRG-bank-0 register (6) with PRG mode bit 6 = 0
    // (meaning $8000 is switchable, $C000/$E000 are fixed at the
    // last two banks).
    out.push(Instruction::new(LDA, AM::Immediate(0x06)));
    out.push(Instruction::new(STA, AM::Absolute(0x8000)));
    out.push(Instruction::new(LDA, AM::Immediate(0x00))); // bank 0 at $8000
    out.push(Instruction::new(STA, AM::Absolute(0x8001)));

    // Select PRG-bank-1 register (7) and load bank 1 at $A000.
    out.push(Instruction::new(LDA, AM::Immediate(0x07)));
    out.push(Instruction::new(STA, AM::Absolute(0x8000)));
    out.push(Instruction::new(LDA, AM::Immediate(0x01)));
    out.push(Instruction::new(STA, AM::Absolute(0x8001)));

    // Mirroring: $A000, bit 0 — 0 = vertical, 1 = horizontal.
    let mirror = match mirroring {
        Mirroring::Horizontal => 0x01,
        Mirroring::Vertical => 0x00,
    };
    out.push(Instruction::new(LDA, AM::Immediate(mirror)));
    out.push(Instruction::new(STA, AM::Absolute(0xA000)));

    // Leave IRQs disabled until the user code enables them.
    out.push(Instruction::new(STA, AM::Absolute(0xE000)));

    out
}

/// Generate the `__bank_select` subroutine. Input: A = desired bank
/// number (0-based, physical PRG bank index). Output: that bank is
/// mapped to $8000-$BFFF. Clobbers A (and the internal shift
/// registers where applicable). The routine ends in RTS so callers
/// can `JSR __bank_select` anywhere it's callable from.
///
/// The bank number is stashed in `ZP_BANK_CURRENT` so `__bank_select`
/// and its trampolines can restore it after a callee returns.
#[must_use]
pub fn gen_bank_select(mapper: Mapper) -> Vec<Instruction> {
    let mut out = Vec::new();
    out.push(Instruction::new(NOP, AM::Label("__bank_select".into())));
    out.push(Instruction::new(STA, AM::ZeroPage(ZP_BANK_CURRENT)));
    match mapper {
        Mapper::NROM => {
            // NROM has no switchable banks, so the routine is a
            // no-op. We still emit it so user code can unconditionally
            // call `__bank_select` regardless of mapper.
            out.push(Instruction::implied(RTS));
        }
        Mapper::MMC1 => {
            // Write 5 bits of A (LSB first) into the shift register
            // at $E000 (PRG-bank select). Between writes we LSR A
            // to shift the next bit into position 0.
            for i in 0..5 {
                if i > 0 {
                    out.push(Instruction::new(LSR, AM::Accumulator));
                }
                out.push(Instruction::new(STA, AM::Absolute(0xE000)));
            }
            out.push(Instruction::implied(RTS));
        }
        Mapper::UxROM => {
            // UxROM: write the bank number to any address in
            // $8000-$FFFF. On boards with bus conflicts the CPU's
            // write and the ROM byte at that address are ANDed on
            // the data bus, so we must write to an address whose
            // ROM byte already equals the bank number. The linker
            // splices a 256-byte table (`__bank_select_table`,
            // bytes 0..255) into the fixed bank, and we index into
            // it with X = bank number: `STA __bank_select_table, X`
            // stores A (= bank number) at
            // `__bank_select_table + X`, whose ROM byte is exactly
            // X, so bus = A = X = ROM — no conflict.
            //
            // Previously this wrote to a fixed `$FFF0`, which
            // happens to work on emulators that don't simulate bus
            // conflicts (jsnes, Mesen permissive) but would glitch
            // on real hardware because a single ROM byte can't
            // match every possible bank number.
            out.push(Instruction::implied(TAX));
            out.push(Instruction::new(
                STA,
                AM::LabelAbsoluteX("__bank_select_table".into()),
            ));
            out.push(Instruction::implied(RTS));
        }
        Mapper::MMC3 => {
            // MMC3: `$8000 = 6` selects PRG-bank-0 register, then
            // write bank to `$8001`. We save/restore X because
            // some callers use X as a loop counter across the
            // switch.
            out.push(Instruction::implied(PHA));
            out.push(Instruction::new(LDA, AM::Immediate(0x06)));
            out.push(Instruction::new(STA, AM::Absolute(0x8000)));
            out.push(Instruction::implied(PLA));
            out.push(Instruction::new(STA, AM::Absolute(0x8001)));
            out.push(Instruction::implied(RTS));
        }
    }
    out
}

/// Generate a cross-bank trampoline stub. Placed in the fixed bank
/// and called by *any* user code via `JSR <tramp_label>` regardless
/// of which bank the caller currently lives in. Behavior:
///
///   1. Read [`ZP_BANK_CURRENT`] into A, push it on the hardware
///      stack — that's the bank we'll need to switch back to.
///   2. Load the target bank number into A, JSR `__bank_select`.
///   3. JSR the user-supplied entry label inside the target bank.
///   4. Pull the saved bank back into A and JSR `__bank_select` to
///      restore the caller's view of $8000-$BFFF.
///   5. RTS.
///
/// The save/restore via `ZP_BANK_CURRENT + PHA/PLA` makes the same
/// trampoline work for **fixed-bank → switchable-bank** *and*
/// **switchable-bank → switchable-bank** call directions: the
/// caller's bank ends up restored regardless of where the call
/// originated. Nested cross-bank calls compose because each
/// trampoline's PHA/PLA pair is balanced against its own JSR/RTS,
/// so the saved bank values stack like any other 6502 frame.
///
/// The trampoline body itself lives in the fixed bank, which is
/// always mapped at `$C000-$FFFF`, so it's reachable from every
/// switchable bank without further mapper trickery.
///
/// `tramp_label` is the label that callers will JSR (the IR codegen
/// emits `JSR __tramp_<fn_name>` at every cross-bank call site).
/// `entry_label` is the label inside the target bank that holds the
/// callee's first instruction — conventionally `__ir_fn_<fn_name>`,
/// the same label IR codegen would have emitted for an in-bank call.
/// `bank_index` is the physical PRG bank number of the target bank.
#[must_use]
pub fn gen_bank_trampoline(
    tramp_label: &str,
    entry_label: &str,
    bank_index: u8,
) -> Vec<Instruction> {
    let mut out = Vec::new();
    out.push(Instruction::new(NOP, AM::Label(tramp_label.to_string())));
    // Save the caller's current bank. `__bank_select` writes its
    // input into ZP_BANK_CURRENT, so this slot already mirrors the
    // last-selected bank (initialized to the fixed bank index by
    // `gen_mapper_init` so even fixed-bank callers see a sane
    // value the first time around).
    out.push(Instruction::new(LDA, AM::ZeroPage(ZP_BANK_CURRENT)));
    out.push(Instruction::implied(PHA));
    // Switch to target bank.
    out.push(Instruction::new(LDA, AM::Immediate(bank_index)));
    out.push(Instruction::new(JSR, AM::Label("__bank_select".into())));
    // Call the user's entry point in that bank. The label lives in
    // the switchable bank and is resolved by the linker after the
    // banked code is assembled.
    out.push(Instruction::new(JSR, AM::Label(entry_label.to_string())));
    // Restore the caller's bank (pulled from the stack) so control
    // returns with $8000-$BFFF showing whatever the caller had
    // mapped before the trampoline ran.
    out.push(Instruction::implied(PLA));
    out.push(Instruction::new(JSR, AM::Label("__bank_select".into())));
    out.push(Instruction::implied(RTS));
    out
}

/// Generate the bus-conflict avoidance table used by `UxROM`. The table
/// lives at a known offset in the fixed bank and contains 256 bytes
/// of increasing values (0, 1, 2, ...). Writing bank `n` to
/// `__bank_select_table + n` guarantees the bus value matches the
/// ROM byte at that address, avoiding conflict-driven glitches on
/// real `UxROM` hardware.
#[must_use]
pub fn gen_uxrom_bank_table() -> Vec<Instruction> {
    let bytes: Vec<u8> = (0..=255u16).map(|i| i as u8).collect();
    vec![
        Instruction::new(NOP, AM::Label("__bank_select_table".into())),
        Instruction::new(NOP, AM::Bytes(bytes)),
    ]
}
