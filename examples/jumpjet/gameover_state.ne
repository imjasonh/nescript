// jumpjet/gameover_state.ne — game over screen.
//
// Big "GAME OVER" banner, the player's final score, a one-shot
// fanfare, and an autopilot timer that loops back to Title so
// the harness can capture multiple boots in a row.

state GameOver {
    var linger: u8 = 0

    on enter {
        linger = 0
        start_music fanfare
    }

    on frame {
        linger += 1

        // GAME OVER banner — two words, centred.
        // "GAME" = 4 letters × 8 = 32 px; "OVER" = 4 letters × 8 = 32 px.
        // Stack them on two rows for emphasis.
        draw_word_game(112, 80)
        draw_word_over(112, 96)

        // Final score below.
        draw_word_score(72, 128)
        draw_score(120, 128)

        if linger >= GAMEOVER_LINGER {
            transition Title
        }
        if button.start {
            transition Title
        }
    }
}
