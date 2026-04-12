// Audio Demo — shows off the minimal APU driver.
//
// Press a button to trigger each sound effect. Start begins the
// background tone; Select stops it. The driver uses pulse 1 for
// SFX and pulse 2 for music, both running out of the built-in
// NMI audio tick.
//
// The SFX name is looked up in a small builtin table of tones:
//     coin    — high short blip (also: pickup, collect)
//     jump    — mid short tone (also: hop)
//     hit     — low short blast (also: damage, explode)
//     click   — short beep (also: select, confirm)
//     cancel  — low longer tone (also: back, error)
//     shoot   — very high short pulse (also: laser, fire)
// Unknown names play a generic mid-frequency beep.
//
// Build:  cargo run -- build examples/audio_demo.ne
// Output: examples/audio_demo.nes

game "Audio Demo" {
    mapper: NROM
}

var px: u8 = 128
var py: u8 = 120
var timer: u8 = 0
var music_on: bool = false

on frame {
    // Move the smiley so you can see the game is running.
    if button.right { px += 1 }
    if button.left  { px -= 1 }
    if button.down  { py += 1 }
    if button.up    { py -= 1 }

    // Trigger SFX on d-pad-press equivalents. Using `_pressed`
    // would be nicer but the builtin table keys off the name, so
    // holding a button just retriggers the same tone every frame,
    // which is audible and good for a demo.
    if button.a { play coin }
    if button.b { play jump }

    // Start/stop the background music loop.
    if button.start { start_music theme }
    if button.select { stop_music }

    // Even without any button presses, the demo auto-plays a
    // short coin SFX every 60 frames so the e2e harness (which
    // runs headless without simulated input) can capture a
    // non-silent audio hash. This exercises the full play-path
    // through the APU driver under CI.
    timer += 1
    if timer == 30 {
        play coin
    }
    if timer == 60 {
        timer = 0
        if music_on {
            stop_music
            music_on = false
        } else {
            start_music theme
            music_on = true
        }
    }

    draw Smiley at: (px, py)
}

start Main
