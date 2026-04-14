use image::GenericImageView;

/// Convert a PNG image to NES CHR tile data (2-bitplane format).
/// Each 8x8 tile = 16 bytes (8 bytes plane 0, 8 bytes plane 1).
pub fn png_to_chr(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let img = image::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let (w, h) = img.dimensions();

    if w % 8 != 0 || h % 8 != 0 {
        return Err(format!("image dimensions {w}x{h} must be multiples of 8"));
    }

    let mut chr_data = Vec::new();
    let tiles_x = w / 8;
    let tiles_y = h / 8;

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let tile = encode_tile(&img, tx * 8, ty * 8);
            chr_data.extend_from_slice(&tile);
        }
    }

    Ok(chr_data)
}

/// Convert a 256×240 PNG into a nametable (`tiles`, `attrs`) pair.
///
/// The image is sliced into 32×30 8×8 cells. Each cell's raw RGB
/// bytes are hashed; the first occurrence of a given hash becomes a
/// fresh tile index. A maximum of 256 unique tiles fit in a single
/// pattern table — anything beyond that is rejected. The 64-byte
/// attribute table is filled by computing, for each 16×16 quadrant of
/// a 32×32 meta-cell, the dominant brightness bucket (0-3) and
/// packing the four buckets into a single byte.
///
/// **Important limitation.** This helper does **not** emit CHR data
/// — the 960-byte tile-index table it produces references tiles
/// assumed to sit at indices 0..N in the user's CHR ROM. Callers
/// typically provide matching CHR via a separate sprite / `@chr(...)`
/// declaration; without that the rendered output won't match the
/// source PNG. The parser warns via the `png_source` flow, the
/// resolver wires it up, and the rest is up to the user for now.
/// Tracked in `docs/future-work.md` as the next increment on this
/// feature.
pub fn png_to_nametable(path: &std::path::Path) -> Result<([u8; 960], [u8; 64]), String> {
    let img = image::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let (w, h) = img.dimensions();
    if w != 256 || h != 240 {
        return Err(format!(
            "nametable PNG {} must be 256×240 (got {w}×{h})",
            path.display()
        ));
    }

    // 32×30 tile grid. For each tile we serialise its 64 pixels into
    // a 192-byte RGB blob, then use that blob as the dedup key via a
    // small hand-rolled table rather than pulling a hash crate in.
    // Keys are kept in a Vec<Vec<u8>> with the index as the tile id
    // — O(N²) in unique tiles, but N ≤ 256 so it's fine.
    let rgb = img.to_rgb8();
    let mut unique_tiles: Vec<Vec<u8>> = Vec::new();
    let mut tiles = [0u8; 960];

    for ty in 0..30u32 {
        for tx in 0..32u32 {
            let mut key = Vec::with_capacity(8 * 8 * 3);
            for row in 0..8u32 {
                for col in 0..8u32 {
                    let p = rgb.get_pixel(tx * 8 + col, ty * 8 + row);
                    key.push(p[0]);
                    key.push(p[1]);
                    key.push(p[2]);
                }
            }
            let idx = if let Some(pos) = unique_tiles.iter().position(|t| t == &key) {
                pos
            } else {
                if unique_tiles.len() >= 256 {
                    return Err(format!(
                        "nametable PNG {} has more than 256 unique 8×8 tiles; \
                         simplify the image or split it into multiple backgrounds",
                        path.display()
                    ));
                }
                unique_tiles.push(key);
                unique_tiles.len() - 1
            };
            tiles[(ty * 32 + tx) as usize] = idx as u8;
        }
    }

    // Attribute table: 8×8 bytes, each covering a 32×32 region made
    // up of four 16×16 quadrants. Each quadrant gets 2 bits
    // (0..=3) packed into the byte as `BR<<6 | BL<<4 | TR<<2 | TL`
    // per the PPU's documented layout. The 15-row nametable only
    // half-fills the last attribute byte-row (rows 8..10 of the
    // bottom attribute byte are unused and stay at 0, matching the
    // hand-packed form the parser already emits).
    //
    // For each 16×16 quadrant we bucket the average brightness of
    // its 256 pixels into 0..=3. That's a crude approximation but
    // it's deterministic and maps "darker" regions to sub-palette 0
    // and "brighter" regions to sub-palette 3 — a reasonable default
    // until per-quadrant palette selection is exposed in the source.
    let mut attrs = [0u8; 64];
    for aty in 0..8u32 {
        for atx in 0..8u32 {
            let quadrant = |qx: u32, qy: u32| -> u8 {
                // qx/qy are 0 or 1 → top-left/top-right/bottom-left/
                // bottom-right of the 32×32 attribute cell.
                let base_x = atx * 32 + qx * 16;
                let base_y = aty * 32 + qy * 16;
                if base_x >= 256 || base_y >= 240 {
                    return 0;
                }
                let mut total: u32 = 0;
                let mut count: u32 = 0;
                let y_end = (base_y + 16).min(240);
                let x_end = (base_x + 16).min(256);
                for y in base_y..y_end {
                    for x in base_x..x_end {
                        let p = rgb.get_pixel(x, y);
                        total += u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
                        count += 1;
                    }
                }
                if count == 0 {
                    return 0;
                }
                let avg = total / (count * 3);
                match avg {
                    0..=63 => 0,
                    64..=127 => 1,
                    128..=191 => 2,
                    _ => 3,
                }
            };
            let tl = quadrant(0, 0);
            let tr = quadrant(1, 0);
            let bl = quadrant(0, 1);
            let br = quadrant(1, 1);
            let byte = (br << 6) | (bl << 4) | (tr << 2) | tl;
            attrs[(aty * 8 + atx) as usize] = byte;
        }
    }

    Ok((tiles, attrs))
}

fn encode_tile(img: &image::DynamicImage, x: u32, y: u32) -> [u8; 16] {
    let mut tile = [0u8; 16];

    for row in 0..8u32 {
        let mut plane0 = 0u8;
        let mut plane1 = 0u8;
        for col in 0..8u32 {
            let pixel = img.get_pixel(x + col, y + row);
            // Map to 2-bit palette index based on brightness
            let brightness = (u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2])) / 3;
            let index = match brightness {
                0..=63 => 0u8,
                64..=127 => 1,
                128..=191 => 2,
                _ => 3,
            };
            if index & 1 != 0 {
                plane0 |= 0x80 >> col;
            }
            if index & 2 != 0 {
                plane1 |= 0x80 >> col;
            }
        }
        tile[row as usize] = plane0;
        tile[row as usize + 8] = plane1;
    }

    tile
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn png_to_nametable_dedupes_tiles() {
        // A 256×240 image split into 8×8 tiles: the top half is all
        // black and the bottom half is all white. We expect the
        // deduplicator to find exactly two unique tiles and to emit
        // a 960-byte tile map where rows 0..14 reference tile 0 and
        // rows 15..29 reference tile 1.
        let dir = std::env::temp_dir();
        let path = dir.join("nescript_png_to_nametable_dedupe.png");
        let mut img = RgbImage::new(256, 240);
        for y in 0..240u32 {
            let c = if y < 120 { 0u8 } else { 255u8 };
            for x in 0..256u32 {
                img.put_pixel(x, y, Rgb([c, c, c]));
            }
        }
        img.save(&path).unwrap();
        let (tiles, attrs) = png_to_nametable(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        // Top 15 rows should be uniformly tile 0; bottom 15 rows
        // should be uniformly tile 1.
        for row in 0..15usize {
            for col in 0..32usize {
                assert_eq!(tiles[row * 32 + col], 0);
            }
        }
        for row in 15..30usize {
            for col in 0..32usize {
                assert_eq!(tiles[row * 32 + col], 1);
            }
        }
        // Attributes: top half dark → sub-palette 0; bottom half
        // bright → sub-palette 3. Each attribute byte covers 32×32
        // so row 0..3 of the attribute table is "top half" and
        // row 4..7 is "bottom half"; row 3 straddles the 120-pixel
        // seam so we only check rows that are cleanly on one side.
        for row in 0..3usize {
            for col in 0..8usize {
                assert_eq!(
                    attrs[row * 8 + col],
                    0,
                    "attr row {row} col {col} should be dark"
                );
            }
        }
        for row in 4..7usize {
            for col in 0..8usize {
                // 3 packed into every 2-bit slot = 0xFF.
                assert_eq!(
                    attrs[row * 8 + col],
                    0xFF,
                    "attr row {row} col {col} should be bright"
                );
            }
        }
    }

    #[test]
    fn png_to_nametable_rejects_wrong_size() {
        let dir = std::env::temp_dir();
        let path = dir.join("nescript_png_nametable_wrong_size.png");
        let img = RgbImage::new(320, 240);
        img.save(&path).unwrap();
        let err = png_to_nametable(&path).unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(err.contains("must be 256"), "unexpected error: {err}");
    }

    #[test]
    fn png_to_nametable_rejects_too_many_unique_tiles() {
        // A 256×240 image of unique gradient tiles — each 8×8 cell
        // has a distinct top-left pixel value. With 32×30 = 960
        // tiles but only 256 unique slots available, this must
        // fail with a clear error. We force uniqueness by tiling
        // monotonically increasing colours across the 32×30 grid.
        let dir = std::env::temp_dir();
        let path = dir.join("nescript_png_nametable_too_many.png");
        let mut img = RgbImage::new(256, 240);
        for ty in 0..30u32 {
            for tx in 0..32u32 {
                let idx = ty * 32 + tx;
                // 960 distinct (r, g, b) triplets. We use 10 bits
                // worth of variation so no two tiles collide.
                let r = (idx & 0xFF) as u8;
                let g = ((idx >> 2) & 0xFF) as u8;
                let b = ((idx >> 4) & 0xFF) as u8;
                for row in 0..8u32 {
                    for col in 0..8u32 {
                        img.put_pixel(tx * 8 + col, ty * 8 + row, Rgb([r, g, b]));
                    }
                }
            }
        }
        img.save(&path).unwrap();
        let err = png_to_nametable(&path).unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(
            err.contains("more than 256 unique"),
            "unexpected error: {err}"
        );
    }
}
