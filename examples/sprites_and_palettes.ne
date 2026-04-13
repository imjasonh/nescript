// Sprites and Scroll — demonstrates M3/M4 asset features.
//
// Shows: sprite declarations with inline CHR data, type casting,
// PPU scroll writes.
//
// Build: cargo run -- build examples/sprites_and_palettes.ne

game "Asset Demo" {
    mapper: NROM
}

// Define a sprite with inline CHR tile data (16 bytes = one 8x8 tile)
// This is a simple arrow pointing right
sprite Arrow {
    chr: [0x18, 0x1C, 0xFE, 0xFF, 0xFF, 0xFE, 0x1C, 0x18,
          0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
}

// Define a sprite with a heart shape
sprite Heart {
    chr: [0x66, 0xFF, 0xFF, 0xFF, 0x7E, 0x3C, 0x18, 0x00,
          0x66, 0xFF, 0xFF, 0xFF, 0x7E, 0x3C, 0x18, 0x00]
}

var px: u8 = 128
var py: u8 = 120
var scroll_x: u8 = 0

on frame {
    // Movement
    if button.right { px += 2 }
    if button.left  { px -= 2 }
    if button.down  { py += 2 }
    if button.up    { py -= 2 }

    // Scroll background
    scroll_x += 1
    scroll(scroll_x, 0)

    // Type cast demo
    var wide: u16 = px as u16

    // Draw sprites
    draw Arrow at: (px, py)
    draw Heart at: (px, py - 16)
}

start Main
