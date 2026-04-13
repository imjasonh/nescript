use std::path::Path;

use crate::linker::SpriteData;
use crate::parser::ast::{AssetSource, Program};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Span;
    use crate::parser::ast::{GameDecl, Mapper, Mirroring, SpriteDecl};

    fn make_program(sprite: SpriteDecl) -> Program {
        Program {
            game: GameDecl {
                name: "Test".to_string(),
                mapper: Mapper::NROM,
                mirroring: Mirroring::Horizontal,
                span: Span::dummy(),
            },
            globals: Vec::new(),
            constants: Vec::new(),
            enums: Vec::new(),
            structs: Vec::new(),
            functions: Vec::new(),
            states: Vec::new(),
            sprites: vec![sprite],
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
}
