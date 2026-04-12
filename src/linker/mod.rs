#[cfg(test)]
mod tests;

use crate::asm;
use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};
use crate::parser::ast::{Mapper, Mirroring};
use crate::rom::RomBuilder;
use crate::runtime;

/// Link compiled code into a complete NES ROM.
pub struct Linker {
    mirroring: Mirroring,
    mapper: Mapper,
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
    pub fn link(&self, user_code: &[Instruction]) -> Vec<u8> {
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

        // NMI handler
        all_instructions.push(Instruction::new(NOP, AM::Label("__nmi".into())));
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

        // Write vector table
        let nmi_addr = result.labels.get("__nmi").copied().unwrap_or(0xC000);
        let reset_addr = result.labels.get("__reset").copied().unwrap_or(0xC000);
        let irq_addr = result.labels.get("__irq").copied().unwrap_or(0xC000);

        prg.extend_from_slice(&nmi_addr.to_le_bytes());
        prg.extend_from_slice(&reset_addr.to_le_bytes());
        prg.extend_from_slice(&irq_addr.to_le_bytes());

        // Build ROM
        let mut builder = RomBuilder::new(self.mirroring);
        builder.set_mapper(crate::rom::mapper_number(self.mapper));
        builder.set_prg(prg);

        // CHR ROM with default sprite tile
        let mut chr = vec![0u8; 8192];
        chr[..16].copy_from_slice(&DEFAULT_SPRITE_CHR);
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
