use std::path::Path;

use crate::linker::SpriteData;
use crate::parser::ast::{AssetSource, Program};

/// Resolved palette data, ready for the linker to splice into PRG
/// ROM as a 32-byte data blob at the label returned by [`Self::label`].
/// Declarations shorter than 32 bytes are zero-padded so the runtime
/// can always push exactly 32 bytes to `$3F00-$3F1F`.
#[derive(Debug, Clone)]
pub struct PaletteData {
    pub name: String,
    /// Exactly 32 bytes. Index `i` is the value written to PPU
    /// address `$3F00 + i`.
    pub colors: [u8; 32],
}

impl PaletteData {
    /// The ROM-level label under which the linker emits the 32-byte
    /// blob. The IR codegen references this label when lowering
    /// `set_palette Name`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("__palette_{}", self.name)
    }
}

/// Resolved background data. `tiles` is the 960-byte nametable
/// (32 columns × 30 rows) and `attrs` is the 64-byte attribute
/// table. Both are zero-padded up from the declared sizes so the
/// runtime NMI helper can always push fixed-length data.
#[derive(Debug, Clone)]
pub struct BackgroundData {
    pub name: String,
    pub tiles: [u8; 960],
    pub attrs: [u8; 64],
}

impl BackgroundData {
    #[must_use]
    pub fn tiles_label(&self) -> String {
        format!("__bg_tiles_{}", self.name)
    }
    #[must_use]
    pub fn attrs_label(&self) -> String {
        format!("__bg_attrs_{}", self.name)
    }
}

/// Resolve sprite declarations in a program into concrete CHR byte blobs and
/// assign each one a tile index in CHR ROM.
///
/// Tile index 0 is reserved for the built-in default smiley sprite, so user
/// sprites start at tile index 1. A single sprite declaration may occupy
/// multiple consecutive tiles if its CHR data is larger than 16 bytes.
///
/// `source_dir` is used as the base for `@binary` / `@chr` relative paths.
/// Missing files are silently skipped (not an error) so programs that
/// reference external assets for documentation purposes compile without
/// requiring the files to exist yet.
pub fn resolve_sprites(program: &Program, source_dir: &Path) -> Result<Vec<SpriteData>, String> {
    let mut sprites = Vec::new();
    // Tile index 0 is the built-in smiley; user sprites start at 1.
    let mut next_tile: u8 = 1;

    for sprite_decl in &program.sprites {
        let chr_bytes = match &sprite_decl.chr_source {
            AssetSource::Inline(bytes) => bytes.clone(),
            AssetSource::Binary(path) => {
                // Try to read raw bytes from the file. Missing files are
                // skipped silently so declarations can reference assets
                // that haven't been added yet.
                let full_path = source_dir.join(path);
                match std::fs::read(&full_path) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                }
            }
            AssetSource::Chr(path) => {
                // PNG → CHR conversion. Missing files skipped silently.
                let full_path = source_dir.join(path);
                match crate::assets::png_to_chr(&full_path) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                }
            }
        };

        // Each NES 8x8 tile is 16 bytes of 2-bitplane CHR data. A single
        // sprite declaration can span multiple tiles when its CHR blob is
        // longer than 16 bytes.
        let tile_count = chr_bytes.len().div_ceil(16);
        if tile_count == 0 {
            continue;
        }
        if next_tile as usize + tile_count > 256 {
            return Err(format!(
                "sprite '{}' would exceed CHR ROM tile limit",
                sprite_decl.name
            ));
        }

        sprites.push(SpriteData {
            name: sprite_decl.name.clone(),
            tile_index: next_tile,
            chr_bytes,
        });
        next_tile += tile_count as u8;
    }

    Ok(sprites)
}

/// Resolve all `palette Name { ... }` declarations in `program` into
/// 32-byte fixed-size blobs suitable for splicing into PRG ROM.
/// Declarations with fewer than 32 colors are zero-padded.
#[must_use]
pub fn resolve_palettes(program: &Program) -> Vec<PaletteData> {
    program
        .palettes
        .iter()
        .map(|p| {
            let mut colors = [0u8; 32];
            for (i, c) in p.colors.iter().enumerate().take(32) {
                colors[i] = *c;
            }
            PaletteData {
                name: p.name.clone(),
                colors,
            }
        })
        .collect()
}

/// Resolve all `background Name { ... }` declarations in `program`
/// into fixed-size 960-byte tile maps and 64-byte attribute tables.
/// Declarations shorter than the maximum are zero-padded.
#[must_use]
pub fn resolve_backgrounds(program: &Program) -> Vec<BackgroundData> {
    program
        .backgrounds
        .iter()
        .map(|b| {
            let mut tiles = [0u8; 960];
            for (i, t) in b.tiles.iter().enumerate().take(960) {
                tiles[i] = *t;
            }
            let mut attrs = [0u8; 64];
            for (i, a) in b.attributes.iter().enumerate().take(64) {
                attrs[i] = *a;
            }
            BackgroundData {
                name: b.name.clone(),
                tiles,
                attrs,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Span;
    use crate::parser::ast::{GameDecl, HeaderFormat, Mapper, Mirroring, SpriteDecl};

    fn make_program(sprite: SpriteDecl) -> Program {
        Program {
            game: GameDecl {
                name: "Test".to_string(),
                mapper: Mapper::NROM,
                mirroring: Mirroring::Horizontal,
                header: HeaderFormat::Ines1,
                span: Span::dummy(),
            },
            globals: Vec::new(),
            constants: Vec::new(),
            enums: Vec::new(),
            structs: Vec::new(),
            functions: Vec::new(),
            states: Vec::new(),
            sprites: vec![sprite],
            palettes: Vec::new(),
            backgrounds: Vec::new(),
            sfx: Vec::new(),
            music: Vec::new(),
            banks: Vec::new(),
            start_state: "Main".to_string(),
            span: Span::dummy(),
        }
    }

    #[test]
    fn resolve_inline_sprite() {
        let sprite = SpriteDecl {
            name: "Player".to_string(),
            chr_source: AssetSource::Inline(vec![0u8; 16]),
            span: Span::dummy(),
        };
        let program = make_program(sprite);
        let sprites = resolve_sprites(&program, Path::new(".")).unwrap();
        assert_eq!(sprites.len(), 1);
        assert_eq!(sprites[0].name, "Player");
        assert_eq!(sprites[0].tile_index, 1);
        assert_eq!(sprites[0].chr_bytes.len(), 16);
    }

    #[test]
    fn resolve_binary_file_reads_bytes() {
        let dir = std::env::temp_dir();
        let file_path = dir.join("nescript_resolve_test.bin");
        let bytes: Vec<u8> = (0x40..0x50).collect();
        std::fs::write(&file_path, &bytes).unwrap();

        let sprite = SpriteDecl {
            name: "Tile".to_string(),
            chr_source: AssetSource::Binary(
                file_path.file_name().unwrap().to_string_lossy().to_string(),
            ),
            span: Span::dummy(),
        };
        let program = make_program(sprite);
        let sprites = resolve_sprites(&program, &dir).unwrap();
        assert_eq!(sprites.len(), 1);
        assert_eq!(sprites[0].chr_bytes, bytes);

        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn resolve_missing_binary_skipped() {
        let sprite = SpriteDecl {
            name: "Missing".to_string(),
            chr_source: AssetSource::Binary("nonexistent.bin".to_string()),
            span: Span::dummy(),
        };
        let program = make_program(sprite);
        let sprites = resolve_sprites(&program, Path::new(".")).unwrap();
        // Missing binary file → silently skipped
        assert!(sprites.is_empty());
    }

    use crate::parser::ast::{BackgroundDecl, PaletteDecl};

    fn blank_program() -> Program {
        Program {
            game: GameDecl {
                name: "Test".to_string(),
                mapper: Mapper::NROM,
                mirroring: Mirroring::Horizontal,
                header: HeaderFormat::Ines1,
                span: Span::dummy(),
            },
            globals: Vec::new(),
            constants: Vec::new(),
            enums: Vec::new(),
            structs: Vec::new(),
            functions: Vec::new(),
            states: Vec::new(),
            sprites: Vec::new(),
            palettes: Vec::new(),
            backgrounds: Vec::new(),
            sfx: Vec::new(),
            music: Vec::new(),
            banks: Vec::new(),
            start_state: "Main".to_string(),
            span: Span::dummy(),
        }
    }

    #[test]
    fn resolve_palette_zero_pads_to_32_bytes() {
        let mut program = blank_program();
        program.palettes.push(PaletteDecl {
            name: "Cool".to_string(),
            colors: vec![0x0F, 0x01, 0x11, 0x21],
            span: Span::dummy(),
        });
        let resolved = resolve_palettes(&program);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "Cool");
        assert_eq!(resolved[0].colors.len(), 32);
        assert_eq!(&resolved[0].colors[..4], &[0x0F, 0x01, 0x11, 0x21]);
        // Remainder is zero-padded.
        assert!(resolved[0].colors[4..].iter().all(|&b| b == 0));
        assert_eq!(resolved[0].label(), "__palette_Cool");
    }

    #[test]
    fn resolve_palette_truncates_beyond_32_bytes() {
        // The analyzer rejects >32-byte palettes with E0201; at the
        // resolve level we defensively truncate so downstream code
        // always sees exactly 32 bytes. This lets bad input still
        // produce a valid ROM structure for diagnostic purposes.
        let mut program = blank_program();
        program.palettes.push(PaletteDecl {
            name: "Big".to_string(),
            colors: (0u8..40).collect(),
            span: Span::dummy(),
        });
        let resolved = resolve_palettes(&program);
        assert_eq!(resolved[0].colors.len(), 32);
        assert_eq!(resolved[0].colors[0], 0);
        assert_eq!(resolved[0].colors[31], 31);
    }

    #[test]
    fn resolve_background_pads_tiles_and_attrs() {
        let mut program = blank_program();
        program.backgrounds.push(BackgroundDecl {
            name: "Stage".to_string(),
            tiles: vec![1, 2, 3],
            attributes: vec![0xFF],
            span: Span::dummy(),
        });
        let resolved = resolve_backgrounds(&program);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "Stage");
        assert_eq!(resolved[0].tiles.len(), 960);
        assert_eq!(resolved[0].tiles[0], 1);
        assert_eq!(resolved[0].tiles[2], 3);
        assert!(resolved[0].tiles[3..].iter().all(|&b| b == 0));
        assert_eq!(resolved[0].attrs.len(), 64);
        assert_eq!(resolved[0].attrs[0], 0xFF);
        assert!(resolved[0].attrs[1..].iter().all(|&b| b == 0));
        assert_eq!(resolved[0].tiles_label(), "__bg_tiles_Stage");
        assert_eq!(resolved[0].attrs_label(), "__bg_attrs_Stage");
    }
}
