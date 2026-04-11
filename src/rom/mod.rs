#[cfg(test)]
mod tests;

use crate::parser::ast::Mirroring;

/// iNES header magic bytes
const INES_MAGIC: [u8; 4] = [0x4E, 0x45, 0x53, 0x1A]; // "NES\x1A"

/// PRG ROM bank size (16 KB)
const PRG_BANK_SIZE: usize = 16384;

/// CHR ROM bank size (8 KB)
const CHR_BANK_SIZE: usize = 8192;

/// Build a complete iNES ROM file.
pub struct RomBuilder {
    prg_data: Vec<u8>,
    chr_data: Vec<u8>,
    mapper: u8,
    mirroring: Mirroring,
}

impl RomBuilder {
    pub fn new(mirroring: Mirroring) -> Self {
        Self {
            prg_data: Vec::new(),
            chr_data: Vec::new(),
            mapper: 0, // NROM
            mirroring,
        }
    }

    #[allow(dead_code)]
    pub fn set_mapper(&mut self, mapper: u8) {
        self.mapper = mapper;
    }

    /// Set the PRG ROM data. Will be padded to fill 16 KB or 32 KB.
    pub fn set_prg(&mut self, data: Vec<u8>) {
        self.prg_data = data;
    }

    /// Set the CHR ROM data. Will be padded to fill 8 KB.
    pub fn set_chr(&mut self, data: Vec<u8>) {
        self.chr_data = data;
    }

    /// Build the complete .nes file.
    pub fn build(self) -> Vec<u8> {
        // Determine PRG size: 1 bank (16 KB) or 2 banks (32 KB)
        let prg_banks = if self.prg_data.len() > PRG_BANK_SIZE {
            2
        } else {
            1
        };
        let prg_size = prg_banks * PRG_BANK_SIZE;

        // CHR: 1 bank (8 KB), or 0 if using CHR RAM
        let chr_banks = usize::from(!self.chr_data.is_empty());
        let chr_size = chr_banks * CHR_BANK_SIZE;

        let mut rom = Vec::with_capacity(16 + prg_size + chr_size);

        // iNES header (16 bytes)
        rom.extend_from_slice(&INES_MAGIC);
        rom.push(prg_banks as u8); // PRG ROM banks (16 KB units)
        rom.push(chr_banks as u8); // CHR ROM banks (8 KB units)

        // Flags 6: mirroring, mapper low nibble
        let mut flags6 = match self.mirroring {
            Mirroring::Horizontal => 0,
            Mirroring::Vertical => 1,
        };
        flags6 |= (self.mapper & 0x0F) << 4;
        rom.push(flags6);

        // Flags 7: mapper high nibble
        let flags7 = self.mapper & 0xF0;
        rom.push(flags7);

        // Bytes 8-15: padding zeros
        rom.extend_from_slice(&[0u8; 8]);

        // PRG ROM data (padded to fill)
        let mut prg = self.prg_data;
        prg.resize(prg_size, 0xFF);
        rom.extend_from_slice(&prg);

        // CHR ROM data (padded to fill)
        if chr_banks > 0 {
            let mut chr = self.chr_data;
            chr.resize(chr_size, 0x00);
            rom.extend_from_slice(&chr);
        }

        rom
    }
}

/// Validate that a byte slice looks like a valid iNES ROM.
pub fn validate_ines(data: &[u8]) -> Result<RomInfo, &'static str> {
    if data.len() < 16 {
        return Err("file too small for iNES header");
    }
    if data[0..4] != INES_MAGIC {
        return Err("invalid iNES magic bytes");
    }

    let prg_banks = data[4] as usize;
    let chr_banks = data[5] as usize;
    let expected_size = 16 + prg_banks * PRG_BANK_SIZE + chr_banks * CHR_BANK_SIZE;

    if data.len() < expected_size {
        return Err("file too small for declared ROM banks");
    }

    let mirroring = if data[6] & 1 == 0 {
        Mirroring::Horizontal
    } else {
        Mirroring::Vertical
    };

    let mapper = (data[6] >> 4) | (data[7] & 0xF0);

    Ok(RomInfo {
        prg_banks,
        chr_banks,
        mapper,
        mirroring,
    })
}

#[derive(Debug)]
pub struct RomInfo {
    pub prg_banks: usize,
    pub chr_banks: usize,
    pub mapper: u8,
    pub mirroring: Mirroring,
}
