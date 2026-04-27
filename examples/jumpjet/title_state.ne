// jumpjet/title_state.ne — title screen.
//
// Big "JUMPJET" banner centred mid-screen, a blinking PRESS START
// prompt below it, music playing on pulse 2, and an autopilot that
// auto-transitions to Playing after TITLE_AUTO_FRAMES frames so
// the headless harness reaches gameplay before frame 180.

state Title {
    var blink: u8 = 0

    on enter {
        blink = 0
        frame_tick = 0
        // A fresh run starts with a full life pool and zero score.
        // The Playing state's `on enter` redoes this anyway, but
        // doing it here means the Title's own HUD (drawn below)
        // reads correctly even before Playing kicks in.
        lives = START_LIVES
        score = 0
        start_music TitleMusic
    }

    on frame {
        frame_tick += 1
        blink += 1
        if blink >= 60 {
            blink = 0
        }

        // Big JUMPJET banner — drawn as sprites so we can use the
        // sprite alphabet tiles without painting any extra
        // background. 7 letters × 8 px = 56 px wide; centred at
        // x = (256 - 56) / 2 = 100.
        draw_word_jumpjet(100, 80)

        // Blink "PRESS START" at roughly 0.5 Hz. Centred under
        // the title banner.
        if blink < 30 {
            draw_word_press(72,  120)
            draw_word_start(120, 120)
        }

        // A demo jet drifting across the title screen so the
        // page never feels empty even when the prompt blinks
        // off. The jet rides the same `frame_tick` waveform as
        // the gameplay autopilot, so the motion is recognisable.
        draw Tileset at: (60 + (frame_tick & 0x1F), 160) frame: TILE_JET_R_TL
        draw Tileset at: (68 + (frame_tick & 0x1F), 160) frame: TILE_JET_R_TR
        draw Tileset at: (60 + (frame_tick & 0x1F), 168) frame: TILE_JET_R_BL
        draw Tileset at: (68 + (frame_tick & 0x1F), 168) frame: TILE_JET_R_BR

        // Auto-advance for the harness; a human can press Start
        // to skip the wait.
        if frame_tick >= TITLE_AUTO_FRAMES {
            transition Playing
        }
        if button.start {
            transition Playing
        }
    }
}
