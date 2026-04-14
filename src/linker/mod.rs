mod debug_symbols;
#[cfg(test)]
mod tests;

pub use debug_symbols::{render_mlb, render_source_map};

use std::collections::HashMap;

use crate::asm;
use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};
use crate::assets::{BackgroundData, MusicData, PaletteData, SfxData};
use crate::parser::ast::{HeaderFormat, Mapper, Mirroring};
use crate::rom::RomBuilder;
use crate::runtime;

/// Detailed result of a link pass. In addition to the final iNES
/// ROM bytes this carries the assembler's symbol table — each label
/// defined anywhere in the assembled fixed bank mapped to its CPU
/// address — and the byte offset at which the fixed bank starts
/// inside the PRG ROM region of the file.
///
/// The CLI uses this metadata to emit Mesen-compatible `.mlb`
/// symbol files and source-to-ROM maps (via the `--symbols` /
/// `--source-map` flags). Callers that only care about the ROM
/// bytes can read `.rom` and discard the rest.
#[derive(Debug, Clone)]
pub struct LinkedRom {
    /// Final iNES ROM bytes (header + PRG banks + CHR).
    pub rom: Vec<u8>,
    /// Every label defined in the fixed bank, mapped to its CPU
    /// address in the $C000-$FFFF window. Populated by the
    /// 6502 assembler's label pass.
    pub labels: HashMap<String, u16>,
    /// Byte offset of the fixed bank's first byte inside `rom`.
    /// For NROM this is `16` (just past the 16-byte iNES header).
    /// For banked mappers each switchable bank shifts it by 16 KB,
    /// so the fixed bank starts at `16 + 16_384 * switchable_bank_count`.
    pub fixed_bank_file_offset: usize,
}

/// Link compiled code into a complete NES ROM.
pub struct Linker {
    mirroring: Mirroring,
    mapper: Mapper,
    header_format: HeaderFormat,
}

/// CHR data for a sprite, placed at a specific tile index in CHR ROM.
#[derive(Debug, Clone)]
pub struct SpriteData {
    pub name: String,
    pub tile_index: u8,
    /// Raw CHR bytes (16 bytes per 8x8 tile).
    pub chr_bytes: Vec<u8>,
}

/// A switchable PRG bank. Each switchable bank occupies a single
/// 16 KB slot in the ROM and can be mapped to $8000-$BFFF at runtime
/// by writing the bank's physical index to the mapper. The linker
/// places switchable banks in declaration order, followed by the
/// fixed bank at the end.
///
/// `entry_label` is the optional trampoline entry point inside this
/// bank — when set, the linker emits a `__tramp_<name>` stub in the
/// fixed bank that selects this bank and JSRs into the label.
/// `data` is raw bytes to splice verbatim (the compiler currently
/// only uses empty data and lets the linker pad with $FF).
#[derive(Debug, Clone)]
pub struct PrgBank {
    pub name: String,
    pub data: Vec<u8>,
    pub entry_label: Option<String>,
}

impl PrgBank {
    /// Create an empty named bank. Convenience for the compiler,
    /// which currently emits all user code into the fixed bank and
    /// just wants switchable slots reserved for future use.
    #[must_use]
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data: Vec::new(),
            entry_label: None,
        }
    }

    /// Create a bank with a raw byte payload and no trampoline entry.
    #[must_use]
    pub fn with_data(name: impl Into<String>, data: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            data,
            entry_label: None,
        }
    }
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
            header_format: HeaderFormat::Ines1,
        }
    }

    pub fn with_mapper(mirroring: Mirroring, mapper: Mapper) -> Self {
        Self {
            mirroring,
            mapper,
            header_format: HeaderFormat::Ines1,
        }
    }

    /// Opt into the NES 2.0 header format for the emitted ROM.
    /// Chainable builder method — returns `self` so callers can
    /// write `Linker::with_mapper(m, p).with_header(HeaderFormat::Nes2)`.
    #[must_use]
    pub fn with_header(mut self, header: HeaderFormat) -> Self {
        self.header_format = header;
        self
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
        self.link_banked(user_code, sprites, sfx, music, &[])
    }

    /// Link with the full asset pipeline plus zero or more
    /// switchable PRG banks. The switchable banks are written in
    /// declaration order and the fixed bank (which contains the
    /// runtime, NMI/IRQ handlers, vector table, bank-select
    /// subroutine, and all user code) is always placed last.
    ///
    /// For mappers that don't support banking (NROM) this is an
    /// error if any switchable banks are supplied. For banked
    /// mappers the linker also splices `gen_mapper_init` into the
    /// reset path and emits a `__bank_select` subroutine plus one
    /// `__tramp_<name>` trampoline for every bank that declares an
    /// `entry_label`.
    pub fn link_banked(
        &self,
        user_code: &[Instruction],
        sprites: &[SpriteData],
        sfx: &[SfxData],
        music: &[MusicData],
        switchable_banks: &[PrgBank],
    ) -> Vec<u8> {
        self.link_banked_with_ppu(user_code, sprites, sfx, music, &[], &[], switchable_banks)
    }

    /// Link with full asset pipeline including palette and
    /// background data blobs. Palettes and backgrounds each emit a
    /// labelled data block inside PRG ROM; the first declared
    /// palette / background is loaded at reset time before
    /// rendering is enabled, and any additional ones become
    /// addressable via `set_palette` / `load_background` (which
    /// queue a vblank-safe write).
    #[allow(clippy::too_many_arguments)]
    pub fn link_banked_with_ppu(
        &self,
        user_code: &[Instruction],
        sprites: &[SpriteData],
        sfx: &[SfxData],
        music: &[MusicData],
        palettes: &[PaletteData],
        backgrounds: &[BackgroundData],
        switchable_banks: &[PrgBank],
    ) -> Vec<u8> {
        self.link_banked_with_ppu_detailed(
            user_code,
            sprites,
            sfx,
            music,
            palettes,
            backgrounds,
            switchable_banks,
        )
        .rom
    }

    /// Like [`Linker::link_banked_with_ppu`] but returns the full
    /// [`LinkedRom`] record, carrying the assembler label table and
    /// the PRG offset of the fixed bank alongside the ROM bytes.
    /// This is the entry point used by the CLI when emitting a
    /// `.mlb` symbol file or a source-map file.
    #[allow(clippy::too_many_arguments)]
    pub fn link_banked_with_ppu_detailed(
        &self,
        user_code: &[Instruction],
        sprites: &[SpriteData],
        sfx: &[SfxData],
        music: &[MusicData],
        palettes: &[PaletteData],
        backgrounds: &[BackgroundData],
        switchable_banks: &[PrgBank],
    ) -> LinkedRom {
        assert!(
            switchable_banks.is_empty() || self.mapper != Mapper::NROM,
            "NROM does not support switchable PRG banks (got {} banks)",
            switchable_banks.len()
        );
        self.link_banked_inner(
            user_code,
            sprites,
            sfx,
            music,
            palettes,
            backgrounds,
            switchable_banks,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn link_banked_inner(
        &self,
        user_code: &[Instruction],
        sprites: &[SpriteData],
        sfx: &[SfxData],
        music: &[MusicData],
        palettes: &[PaletteData],
        backgrounds: &[BackgroundData],
        switchable_banks: &[PrgBank],
    ) -> LinkedRom {
        // ROM layout.
        //
        // NROM: a single 16 KB PRG bank mapped at $C000-$FFFF.
        //
        // Banked (MMC1, UxROM, MMC3): `switchable_banks` switchable
        // 16 KB banks come first in physical order, followed by the
        // fixed bank. The fixed bank holds the runtime, NMI/IRQ
        // handlers, user code, bank-select routine, and all
        // trampolines — everything needed for control flow to work
        // at reset. The mapper is configured so the fixed bank
        // maps to $C000-$FFFF and one of the switchable banks maps
        // to $8000-$BFFF.
        let total_banks = switchable_banks.len() + 1;
        let fixed_bank_index = total_banks - 1;

        let mut all_instructions = Vec::new();

        // RESET entry point
        all_instructions.push(Instruction::new(NOP, AM::Label("__reset".into())));

        // Hardware initialization
        all_instructions.extend(runtime::gen_init());

        // Mapper configuration: for banked mappers, set up the PRG
        // layout so the fixed bank sits at $C000-$FFFF. NROM is a
        // no-op here.
        all_instructions.extend(runtime::gen_mapper_init(
            self.mapper,
            self.mirroring,
            total_banks,
        ));

        // Load the initial palette. If the program declared any
        // `palette` blocks, use the first one; otherwise fall back
        // to the built-in default palette so sprites show up in a
        // reasonable colour scheme without any user setup.
        //
        // IMPORTANT: `gen_init` leaves rendering fully disabled so
        // these $2006/$2007 writes are safe. We re-enable rendering
        // via `gen_enable_rendering` once all initial VRAM loads
        // complete — writing to $2007 with either the sprite or the
        // background layer active corrupts the PPU's internal
        // address register, which used to clobber everything past
        // about the first 72 bytes of a 1024-byte nametable load.
        if let Some(first_palette) = palettes.first() {
            all_instructions.extend(runtime::gen_initial_palette_load(&first_palette.label()));
        } else {
            all_instructions.extend(self.gen_palette_load());
        }

        // Load the initial background if the program declared any.
        // Most programs don't, so the common case emits nothing
        // here and leaves nametable 0 zero-filled.
        let has_user_background = !backgrounds.is_empty();
        if let Some(first_bg) = backgrounds.first() {
            all_instructions.extend(runtime::gen_initial_background_load(
                &first_bg.tiles_label(),
                &first_bg.attrs_label(),
            ));
        }

        // Now that all palette and nametable writes are done, turn
        // rendering on. Programs with a declared background get
        // bg+sprites ($1E); programs without get sprites only ($10)
        // to preserve the pre-fix behaviour of example ROMs that
        // rely on a hidden nametable.
        all_instructions.extend(runtime::gen_enable_rendering(has_user_background));

        // User code (var init + main loop)
        all_instructions.extend(user_code.iter().cloned());

        // Bank-select subroutine plus a trampoline per declared bank
        // that has an entry label. Emitted only for banked mappers
        // (NROM has no switchable banks by definition). The helpers
        // live in the fixed bank so they're always reachable at
        // $C000-$FFFF regardless of which switchable bank is
        // currently mapped at $8000.
        if self.mapper != Mapper::NROM {
            all_instructions.extend(runtime::gen_bank_select(self.mapper));
            #[allow(clippy::cast_possible_truncation)]
            let fixed_bank_num = fixed_bank_index as u8;
            for (i, bank) in switchable_banks.iter().enumerate() {
                if let Some(entry) = &bank.entry_label {
                    #[allow(clippy::cast_possible_truncation)]
                    let bank_num = i as u8;
                    all_instructions.extend(runtime::gen_bank_trampoline(
                        &bank.name,
                        entry,
                        bank_num,
                        fixed_bank_num,
                    ));
                }
            }
            if self.mapper == Mapper::UxROM {
                // UxROM needs a 256-byte bank-select bus-conflict
                // table in the fixed bank. The `__bank_select`
                // routine for UxROM writes to $FFF0 so the byte
                // at that address in ROM must match the bank being
                // selected — we splice in a 0..255 table just before
                // the vector area.
                all_instructions.extend(runtime::gen_uxrom_bank_table());
            }
        }

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
        let has_noise = has_label(user_code, "__noise_used");
        let has_triangle = has_label(user_code, "__triangle_used");
        if has_audio {
            all_instructions.extend(runtime::gen_audio_tick(has_noise, has_triangle));
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

        // Palette and background data blobs. Each palette is a
        // 32-byte block labelled `__palette_Name`; backgrounds are
        // split into two blocks (`__bg_tiles_Name`, `__bg_attrs_Name`)
        // so the reset loader and the NMI update helper can push
        // them with independent pointers. We always splice the
        // blobs whenever the program declares any palette or
        // background — there's no equivalent of `__audio_used`
        // because simply *declaring* a palette is enough to need
        // its bytes in ROM (the reset loader reads them).
        for pal in palettes {
            all_instructions.extend(runtime::gen_data_block(&pal.label(), pal.colors.to_vec()));
        }
        for bg in backgrounds {
            all_instructions.extend(runtime::gen_data_block(
                &bg.tiles_label(),
                bg.tiles.to_vec(),
            ));
            all_instructions.extend(runtime::gen_data_block(
                &bg.attrs_label(),
                bg.attrs.to_vec(),
            ));
        }

        // The NMI needs the palette/nametable update helper whenever
        // the program declared any palette or background, or the
        // IR codegen emitted the `__ppu_update_used` marker (which
        // signals that user code contains a `set_palette` or
        // `load_background` statement). Either condition brings in
        // the ~70-byte helper; programs that touch neither pay
        // zero bytes.
        let has_ppu_updates = !palettes.is_empty()
            || !backgrounds.is_empty()
            || has_label(user_code, "__ppu_update_used");

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
        // The audio tick JSR is emitted by `gen_nmi` itself, after
        // the register and scratch-slot saves, so it can freely
        // clobber A/X/Y and $02/$03 without corrupting user state.
        // The codegen emits a `__debug_mode` marker whenever
        // `--debug` is active; that tells the runtime to splice
        // in the extra frame-overrun check at the top of NMI.
        let debug_mode = has_label(user_code, "__debug_mode");
        all_instructions.extend(runtime::gen_nmi(has_ppu_updates, has_audio, debug_mode));

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
        if self.header_format == HeaderFormat::Nes2 {
            builder.enable_nes2();
        }

        // Multi-bank layout: each switchable bank is an independent
        // 16 KB slot whose contents the linker takes verbatim from
        // the caller, followed by the fixed bank (just assembled).
        // For NROM (no switchable banks) this collapses to the
        // legacy single-bank path.
        if switchable_banks.is_empty() {
            builder.set_prg(prg);
        } else {
            let mut banks: Vec<Vec<u8>> = Vec::with_capacity(total_banks);
            for bank in switchable_banks {
                assert!(
                    bank.data.len() <= 16384,
                    "switchable bank '{}' exceeds 16 KB ({} bytes)",
                    bank.name,
                    bank.data.len()
                );
                banks.push(bank.data.clone());
            }
            banks.push(prg);
            builder.set_prg_banks(banks);
        }

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

        let rom = builder.build();
        // The fixed bank sits after the iNES header (16 bytes) and
        // any switchable banks (16 KB each). Callers use this to
        // translate CPU addresses from `labels` into ROM file
        // offsets when emitting Mesen `.mlb` files.
        let fixed_bank_file_offset = 16 + switchable_banks.len() * 16_384;
        LinkedRom {
            rom,
            labels: result.labels,
            fixed_bank_file_offset,
        }
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
