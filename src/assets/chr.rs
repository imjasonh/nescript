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
