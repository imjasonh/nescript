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
/// For now, only [`AssetSource::Inline`] sources produce sprite data; file-
/// backed sources are parsed but skipped here (future work), which keeps
/// existing tests that reference missing files compiling without I/O errors.
pub fn resolve_sprites(program: &Program, source_dir: &Path) -> Result<Vec<SpriteData>, String> {
    let _ = source_dir; // reserved for future file-backed resolution
    let mut sprites = Vec::new();
    // Tile index 0 is the built-in smiley; user sprites start at 1.
    let mut next_tile: u8 = 1;

    for sprite_decl in &program.sprites {
        let chr_bytes = match &sprite_decl.chr_source {
            AssetSource::Inline(bytes) => bytes.clone(),
            // Binary/Chr loading from files is future work; skip for now so
            // programs that reference (possibly-missing) external assets
            // still compile without I/O errors.
            AssetSource::Binary(_) | AssetSource::Chr(_) => continue,
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
