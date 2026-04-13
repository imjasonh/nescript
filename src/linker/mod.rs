#[cfg(test)]
mod tests;

use crate::asm;
use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};
use crate::assets::{MusicData, SfxData};
use crate::parser::ast::{Mapper, Mirroring};
use crate::rom::RomBuilder;
use crate::runtime;

/// Link compiled code into a complete NES ROM.
pub struct Linker {
    mirroring: Mirroring,
    mapper: Mapper,
}

/// CHR data for a sprite, placed at a specific tile index in CHR ROM.
#[derive(Debug, Clone)]
pub struct SpriteData {
    pub name: String,
    pub tile_index: u8,
    /// Raw CHR bytes (16 bytes per 8x8 tile).
    pub chr_bytes: Vec<u8>,
}

/// True if `instructions` contains a label definition with the given
/// name. Labels are emitted as `NOP` pseudo-instructions whose mode
/// is `AddressingMode::Label(name)`.
fn has_label(instructions: &[Instruction], name: &str) -> bool {
    instructions
        .iter()
        .any(|i| matches!(&i.mode, AM::Label(n) if n == name))
}

/// A smiley face CHR tile for the default sprite (M1).
const DEFAULT_SPRITE_CHR: [u8; 16] = [
    // Plane 0 (low bits)
    0b0011_1100,
    0b0100_0010,
    0b1010_0101,
    0b1000_0001,
    0b1010_0101,
    0b1001_1001,
    0b0100_0010,
    0b0011_1100,
    // Plane 1 (high bits) — all zeros means color 1 only
    0b0011_1100,
    0b0111_1110,
    0b1111_1111,
    0b1111_1111,
    0b1111_1111,
    0b1111_1111,
    0b0111_1110,
    0b0011_1100,
];

/// Default palette data for M1 (writes to PPU $3F00).
const DEFAULT_PALETTE: [u8; 32] = [
    // Background palettes
    0x0F, 0x00, 0x10, 0x20, // palette 0 (black, dark gray, light gray, white)
    0x0F, 0x06, 0x16, 0x26, // palette 1
    0x0F, 0x09, 0x19, 0x29, // palette 2
    0x0F, 0x01, 0x11, 0x21, // palette 3
    // Sprite palettes
    0x0F, 0x00, 0x10, 0x20, // sprite palette 0 (same as bg)
    0x0F, 0x14, 0x24, 0x34, // sprite palette 1
    0x0F, 0x1A, 0x2A, 0x3A, // sprite palette 2
    0x0F, 0x12, 0x22, 0x32, // sprite palette 3
];

impl Linker {
    pub fn new(mirroring: Mirroring) -> Self {
        Self {
            mirroring,
            mapper: Mapper::NROM,
        }
    }

    pub fn with_mapper(mirroring: Mirroring, mapper: Mapper) -> Self {
        Self { mirroring, mapper }
    }

    /// Link all code sections into a .nes ROM.
    ///
    /// This is a thin wrapper around [`Linker::link_with_assets`] that passes
    /// an empty sprite list, so the CHR ROM only contains the default smiley
    /// tile at index 0.
    pub fn link(&self, user_code: &[Instruction]) -> Vec<u8> {
        self.link_with_assets(user_code, &[])
    }

    /// Link all code sections into a .nes ROM, placing sprite CHR data at
    /// specific tile indices. No audio data is linked — use
    /// [`Linker::link_with_all_assets`] for audio.
    pub fn link_with_assets(&self, user_code: &[Instruction], sprites: &[SpriteData]) -> Vec<u8> {
        self.link_with_all_assets(user_code, sprites, &[], &[])
    }

    /// Link all code sections into a .nes ROM, placing both graphic
    /// assets (sprite CHR) and audio assets (sfx envelopes, music
    /// note streams) into the appropriate ROM regions.
    ///
    /// Audio data is spliced into PRG ROM under labels derived from
    /// each blob's name (see `SfxData::label` / `MusicData::label`).
    /// The linker only emits these blobs and the audio-driver body
    /// when user code contains the `__audio_used` marker label, so
    /// programs that never touch audio pay zero ROM cost.
    pub fn link_with_all_assets(
        &self,
        user_code: &[Instruction],
        sprites: &[SpriteData],
        sfx: &[SfxData],
        music: &[MusicData],
    ) -> Vec<u8> {
        // For NROM: everything fits in one 16 KB PRG bank ($C000-$FFFF)
        // Layout:
        //   $C000: RESET handler (init + palette load + user code)
        //   ...  : NMI handler
        //   ...  : IRQ handler
        //   $FFFA: Vector table (NMI, RESET, IRQ)

        let mut all_instructions = Vec::new();

        // RESET entry point
        all_instructions.push(Instruction::new(NOP, AM::Label("__reset".into())));

        // Hardware initialization
        all_instructions.extend(runtime::gen_init());

        // Load default palette
        all_instructions.extend(self.gen_palette_load());

        // User code (var init + main loop)
        all_instructions.extend(user_code.iter().cloned());

        // Math runtime routines (included always for simplicity)
        all_instructions.extend(runtime::gen_multiply());
        all_instructions.extend(runtime::gen_divide());

        // Audio subsystem — linked in whenever user code touched
        // audio (detected via the `__audio_used` marker emitted by
        // the IR codegen). The driver body, period table, and
        // user/builtin data blobs are all spliced into PRG here.
        //
        // Order is important: the audio tick references both the
        // period table and the data blobs by label, so those labels
        // must be defined in the same assembly pass. The tick body
        // also has to exist before `__nmi` because NMI JSRs into
        // `__audio_tick` — so we emit it alongside the math
        // routines, well before the NMI handler below.
        let has_audio = has_label(user_code, "__audio_used");
        if has_audio {
            all_instructions.extend(runtime::gen_audio_tick());
            all_instructions.extend(runtime::gen_period_table());
            // Emit one data block per sfx blob: a label followed by
            // the envelope bytes. `play Name` codegen emits a
            // SymbolLo/SymbolHi pair that resolves to this label.
            for blob in sfx {
                all_instructions.extend(runtime::gen_data_block(
                    &blob.label(),
                    blob.envelope.clone(),
                ));
            }
            // Same for music: label + note stream.
            for blob in music {
                all_instructions
                    .extend(runtime::gen_data_block(&blob.label(), blob.stream.clone()));
            }
        }

        // NMI handler
        all_instructions.push(Instruction::new(NOP, AM::Label("__nmi".into())));
        // If user code emits an MMC3 reload hook, splice in a JSR
        // before the regular NMI runs. This reloads the scanline IRQ
        // counter each frame so the handler fires at the right line.
        // The presence of the `__ir_mmc3_reload` label is detected
        // during assembly via the labels map; we unconditionally
        // emit a conditional JSR whose target is resolved at link
        // time. The helper emits an RTS so it's safe to call even
        // when there's no work to do.
        if has_label(user_code, "__ir_mmc3_reload") {
            all_instructions.push(Instruction::new(JSR, AM::Label("__ir_mmc3_reload".into())));
        }
        // Audio tick: if audio is in use, JSR into the per-frame
        // driver tick before the normal NMI body. The tick walks
        // both the sfx envelope and the music note stream, writing
        // APU registers as needed. Programs that never use audio
        // skip this splice entirely — no ROM cost.
        if has_audio {
            all_instructions.push(Instruction::new(JSR, AM::Label("__audio_tick".into())));
        }
        all_instructions.extend(runtime::gen_nmi());

        // IRQ handler
        all_instructions.push(Instruction::new(NOP, AM::Label("__irq".into())));
        all_instructions.extend(runtime::gen_irq());

        // Assemble everything at $C000
        let base_addr = 0xC000;
        let result = asm::assemble(&all_instructions, base_addr);

        // Build PRG ROM with vector table
        let mut prg = result.bytes;

        // Pad to fill the bank up to vector table location
        // Vector table is at $FFFA-$FFFF (relative offset: $3FFA in a 16 KB bank)
        let vector_offset = 0x3FFA;
        if prg.len() > vector_offset {
            panic!("PRG code exceeds 16 KB bank (code is {} bytes)", prg.len());
        }
        prg.resize(vector_offset, 0xFF);

        // Write vector table. IR codegen emits a richer IRQ handler
        // under `__irq_user` when the program has scanline handlers;
        // prefer that over the generic RTI stub at `__irq`.
        let nmi_addr = result.labels.get("__nmi").copied().unwrap_or(0xC000);
        let reset_addr = result.labels.get("__reset").copied().unwrap_or(0xC000);
        let irq_addr = result
            .labels
            .get("__irq_user")
            .or_else(|| result.labels.get("__irq"))
            .copied()
            .unwrap_or(0xC000);

        prg.extend_from_slice(&nmi_addr.to_le_bytes());
        prg.extend_from_slice(&reset_addr.to_le_bytes());
        prg.extend_from_slice(&irq_addr.to_le_bytes());

        // Build ROM
        let mut builder = RomBuilder::new(self.mirroring);
        builder.set_mapper(crate::rom::mapper_number(self.mapper));
        builder.set_prg(prg);

        // CHR ROM: tile 0 is reserved for the default smiley, followed by
        // any user-declared sprites placed at their assigned tile indices.
        let mut chr = vec![0u8; 8192];
        chr[..16].copy_from_slice(&DEFAULT_SPRITE_CHR);
        for sprite in sprites {
            let offset = sprite.tile_index as usize * 16;
            let end = offset + sprite.chr_bytes.len();
            if end <= chr.len() {
                chr[offset..end].copy_from_slice(&sprite.chr_bytes);
            }
        }
        builder.set_chr(chr);

        builder.build()
    }

    /// Generate instructions to load the default palette into the PPU.
    fn gen_palette_load(&self) -> Vec<Instruction> {
        let mut out = Vec::new();

        // Set PPU address to $3F00 (palette start)
        out.push(Instruction::new(LDA, AM::Absolute(0x2002))); // read PPU status to reset latch
        out.push(Instruction::new(LDA, AM::Immediate(0x3F)));
        out.push(Instruction::new(STA, AM::Absolute(0x2006))); // PPU addr high byte
        out.push(Instruction::new(LDA, AM::Immediate(0x00)));
        out.push(Instruction::new(STA, AM::Absolute(0x2006))); // PPU addr low byte

        // Write all 32 palette bytes
        for &color in &DEFAULT_PALETTE {
            out.push(Instruction::new(LDA, AM::Immediate(color)));
            out.push(Instruction::new(STA, AM::Absolute(0x2007))); // PPU data
        }

        out
    }
}
