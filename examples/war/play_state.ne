// war/play_state.ne — the Playing state and its inner phase machine.
//
// `phase` cycles through the P_* constants defined in
// war/constants.ne. `phase_timer` counts frames inside the
// current phase and is reset to 0 whenever the phase changes.
//
// All function-local var names are prefixed with the function's
// short name (or with `pf_` for "play frame") so the global
// symbol table stays collision-free.

// Set the current phase and zero the timer in one shot. Inlined
// so each call site is just two stores.
inline fun set_phase(p: u8) {
    phase = p
    phase_timer = 0
}

// Draw the steady-state table furniture: the two deck card backs
// (if non-empty) and the running card counts. Used as the base
// layer every frame; phase-specific sprites are layered on top.
fun draw_table() {
    if deck_a_count > 0 {
        draw_card_back(DECK_A_X, DECK_Y)
    }
    if deck_b_count > 0 {
        draw_card_back(DECK_B_X, DECK_Y)
    }
    draw_count(COUNT_A_X, COUNT_Y, deck_a_count)
    draw_count(COUNT_B_X, COUNT_Y, deck_b_count)
}

// Bury helper for a war: move one card from the deck into the
// pot (face-down). Must be called with a non-empty deck. Two
// near-identical helpers (one per side) keep the locals
// uniquely-named.
fun bury_from_a() {
    var bfa_c: u8 = draw_front_a()
    push_back_pot(bfa_c)
}
fun bury_from_b() {
    var bfb_c: u8 = draw_front_b()
    push_back_pot(bfb_c)
}

// Draw the BIG WAR banner — three 16x16 metasprites at the
// centre of the screen. 12 sprites total, drawn in the centre
// row so they don't conflict with the deck stacks (rows 64-87)
// or the face-up cards (rows 128-151).
//
// All offsets stepped through locals to dodge the `x + N`
// parameter aliasing bug; see draw_word_player.
fun draw_big_war_banner(x: u8, y: u8) {
    var bwb_y1: u8 = y + 8
    var bwb_x1: u8 = x + 8
    var bwb_x2: u8 = x + 20
    var bwb_x3: u8 = x + 28
    var bwb_x4: u8 = x + 40
    var bwb_x5: u8 = x + 48
    // BIG W
    draw Tileset at: (x,      y)      frame: TILE_BIG_W_TL
    draw Tileset at: (bwb_x1, y)      frame: TILE_BIG_W_TR
    draw Tileset at: (x,      bwb_y1) frame: TILE_BIG_W_BL
    draw Tileset at: (bwb_x1, bwb_y1) frame: TILE_BIG_W_BR
    // BIG A
    draw Tileset at: (bwb_x2, y)      frame: TILE_BIG_A_TL
    draw Tileset at: (bwb_x3, y)      frame: TILE_BIG_A_TR
    draw Tileset at: (bwb_x2, bwb_y1) frame: TILE_BIG_A_BL
    draw Tileset at: (bwb_x3, bwb_y1) frame: TILE_BIG_A_BR
    // BIG R
    draw Tileset at: (bwb_x4, y)      frame: TILE_BIG_R_TL
    draw Tileset at: (bwb_x5, y)      frame: TILE_BIG_R_TR
    draw Tileset at: (bwb_x4, bwb_y1) frame: TILE_BIG_R_BL
    draw Tileset at: (bwb_x5, bwb_y1) frame: TILE_BIG_R_BR
}

// Begin the A-side draw animation: pull the top card off deck_a,
// stash it as the face-up `card_a`, arm the fly state for the
// deck → play slide, and play the click sfx.
//
// fly_card / fly_face_up are stuffed directly into globals
// instead of being passed to arm_fly, because arm_fly only takes
// 4 params (the v0.1 ABI limit) and silently drops anything past
// the fourth.
fun begin_draw_a() {
    if deck_a_count > 0 {
        card_a = draw_front_a()
        fly_card = card_a
        fly_face_up = 1
        // dx_sign 0 = move right (DECK_A_X = 32 → PLAY_A_X = 96).
        // dy_sign 0 = move down  (DECK_Y = 64 → PLAY_Y = 128).
        arm_fly(DECK_A_X, DECK_Y, 0, 0)
        play FlipCard
        set_phase(P_FLY_A)
    }
}

// Begin the B-side draw. dx_sign 1 = move left
// (DECK_B_X = 208 → PLAY_B_X = 144). dy_sign 0 = move down.
fun begin_draw_b() {
    if deck_b_count > 0 {
        card_b = draw_front_b()
        fly_card = card_b
        fly_face_up = 1
        arm_fly(DECK_B_X, DECK_Y, 1, 0)
        play FlipCard
        set_phase(P_FLY_B)
    }
}

state Playing {
    on enter {
        set_phase(P_WAIT_A)
        card_a = 0
        card_b = 0
        pot_count = 0
    }

    on frame {
        global_tick += 1
        phase_timer += 1
        draw_table()

        // ── Phase dispatch ───────────────────────────────
        // The phases share a simple "timer hits target, advance"
        // shape. Each arm of the if-chain is self-contained and
        // ends either with a set_phase call or a fall-through
        // that waits for more input / time.

        if phase == P_WAIT_A {
            // Human prompt: hint blink above the deck.
            if a_is_cpu == 0 {
                if (phase_timer & 32) == 0 {
                    draw_word_press(8, 200)
                }
                if button.a or button.start {
                    begin_draw_a()
                }
            } else {
                // CPU draws after a short delay.
                if phase_timer >= CPU_THINK_FRAMES {
                    begin_draw_a()
                }
            }
        }

        if phase == P_FLY_A {
            step_fly_pos()
            draw_flying_card(fly_x, fly_y)
            if phase_timer >= FRAMES_FLY {
                set_phase(P_WAIT_B)
            }
        }

        if phase == P_WAIT_B {
            // A's card is now parked in its play slot.
            draw_card_face(PLAY_A_X, PLAY_Y, card_a)
            if b_is_cpu == 0 {
                if (phase_timer & 32) == 0 {
                    draw_word_press(208, 200)
                }
                if p2.button.a or p2.button.start or button.a or button.start {
                    begin_draw_b()
                }
            } else {
                if phase_timer >= CPU_THINK_FRAMES {
                    begin_draw_b()
                }
            }
        }

        if phase == P_FLY_B {
            // A is in place; B is flying.
            draw_card_face(PLAY_A_X, PLAY_Y, card_a)
            step_fly_pos()
            draw_flying_card(fly_x, fly_y)
            if phase_timer >= FRAMES_FLY {
                set_phase(P_REVEAL)
            }
        }

        if phase == P_REVEAL {
            draw_card_face(PLAY_A_X, PLAY_Y, card_a)
            draw_card_face(PLAY_B_X, PLAY_Y, card_b)
            if phase_timer >= FRAMES_REVEAL {
                set_phase(P_RESOLVE)
            }
        }

        if phase == P_RESOLVE {
            // Both cards go into the pot regardless of outcome.
            push_back_pot(card_a)
            push_back_pot(card_b)
            var pf_r: u8 = compare_cards(card_a, card_b)
            if pf_r == 1 {
                play CheerA
                set_phase(P_WIN_A)
            }
            if pf_r == 2 {
                play CheerB
                set_phase(P_WIN_B)
            }
            if pf_r == 0 {
                // It's a tie — but only enter the war flow if both
                // sides actually have cards left to bury. If a
                // player ran out of cards on this very tie, the
                // OTHER player wins by default and takes the pot.
                if deck_a_count == 0 {
                    pot_to_b()
                    winner = 1
                    transition Victory
                }
                if deck_b_count == 0 {
                    pot_to_a()
                    winner = 0
                    transition Victory
                }
                play WarFlash
                set_phase(P_WAR_BANNER)
            }
        }

        if phase == P_WIN_A {
            draw_card_face(PLAY_A_X, PLAY_Y, card_a)
            draw_card_face(PLAY_B_X, PLAY_Y, card_b)
            if phase_timer >= FRAMES_FLY {
                pot_to_a()
                set_phase(P_CHECK)
            }
        }

        if phase == P_WIN_B {
            draw_card_face(PLAY_A_X, PLAY_Y, card_a)
            draw_card_face(PLAY_B_X, PLAY_Y, card_b)
            if phase_timer >= FRAMES_FLY {
                pot_to_b()
                set_phase(P_CHECK)
            }
        }

        if phase == P_WAR_BANNER {
            draw_card_face(PLAY_A_X, PLAY_Y, card_a)
            draw_card_face(PLAY_B_X, PLAY_Y, card_b)
            // Flashing big "WAR" banner — only drawn on alternate
            // 8-frame windows so the title strobes for emphasis.
            if (phase_timer & 8) != 0 {
                draw_big_war_banner(96, 80)
            }
            if phase_timer >= FRAMES_BANNER {
                set_phase(P_WAR_BURY)
            }
        }

        if phase == P_WAR_BURY {
            // Bury up to 3 face-down cards from each deck, then
            // draw a new face-up pair. We don't animate each
            // individual buried card; just play a noise thump
            // per buried card and advance the counters.
            if phase_timer == 1 {
                if deck_a_count > 0 { bury_from_a() }
                if deck_b_count > 0 { bury_from_b() }
                play ThudDown
            }
            if phase_timer == 4 {
                if deck_a_count > 0 { bury_from_a() }
                if deck_b_count > 0 { bury_from_b() }
                play ThudDown
            }
            if phase_timer == 7 {
                if deck_a_count > 0 { bury_from_a() }
                if deck_b_count > 0 { bury_from_b() }
                play ThudDown
            }
            if phase_timer == 10 {
                // Draw new face-ups for the comparison. If either
                // side has run out of cards, the OTHER side wins
                // and takes the entire pot — we transition straight
                // to Victory.
                if deck_a_count == 0 {
                    pot_to_b()
                    winner = 1
                    transition Victory
                }
                if deck_b_count == 0 {
                    pot_to_a()
                    winner = 0
                    transition Victory
                }
                card_a = draw_front_a()
                card_b = draw_front_b()
            }
            if phase_timer >= FRAMES_BURY + 16 {
                set_phase(P_REVEAL)
            }
        }

        if phase == P_CHECK {
            if deck_a_count == 0 {
                winner = 1
                transition Victory
            }
            if deck_b_count == 0 {
                winner = 0
                transition Victory
            }
            // No winner yet — start the next round.
            card_a = 0
            card_b = 0
            set_phase(P_WAIT_A)
        }
    }
}
