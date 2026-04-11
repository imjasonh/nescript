// Hello Sprite — the simplest NEScript program.
//
// Displays a smiley-face sprite on screen and moves it with the d-pad.
// Build:  cargo run -- build examples/hello_sprite.ne
// Output: examples/hello_sprite.nes (open in any NES emulator)

game "Hello Sprite" {
    mapper: NROM
}

var px: u8 = 128
var py: u8 = 120

on frame {
    if button.right { px += 2 }
    if button.left  { px -= 2 }
    if button.down  { py += 2 }
    if button.up    { py -= 2 }

    draw Smiley at: (px, py)
}

start Main
