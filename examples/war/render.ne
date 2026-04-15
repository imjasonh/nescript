// war/render.ne — card, digit, and text rendering helpers.
//
// Every function in this file is a thin wrapper around one or
// more `draw Tileset at: (x, y) frame: N` calls that writes to
// the runtime OAM cursor. Each `draw` takes one sprite slot, so
// a 16×24 card face burns 6 sprite slots, a single-character
// letter burns 1, and so on. The caller is responsible for
// sprite budgeting — see PLAN.md §3.
//
// Function-local var names are prefixed with the function's
// short name to avoid the global symbol-table collisions that
// E0501 would otherwise complain about.

// ── Card face ──────────────────────────────────────────────
//
// Draws the 6-sprite card face at (x, y) for the packed card
// byte. Layout (each cell is 8×8):
//
//     [rank  ][small_suit]   row 0
//     [pipL  ][pipR      ]   row 1
//     [blankL][blankR    ]   row 2
//
// Rank is the condensed glyph at TILE_RANK_BASE + (rank - 1);
// small suit + big-pip halves are indexed directly off their
// base constants.
fun draw_card_face(dcf_in_x: u8, dcf_in_y: u8, dcf_in_card: u8) {
    // Snapshot every parameter into a fresh local *before* any
    // nested function call. NEScript v0.1 passes parameters via
    // fixed zero-page slots ($04, $05, $06), and any inner call
    // overwrites those slots with its own parameter values —
    // including the inline `card_rank` / `card_suit` calls below,
    // which would otherwise leave `x` and `y` corrupted by the
    // time the draw lines run.
    var dcf_x:    u8 = dcf_in_x
    var dcf_y:    u8 = dcf_in_y
    var dcf_card: u8 = dcf_in_card
    var dcf_rank: u8 = card_rank(dcf_card)
    var dcf_suit: u8 = card_suit(dcf_card)
    var dcf_rank_tile:  u8 = TILE_RANK_BASE + dcf_rank - 1
    var dcf_small_tile: u8 = TILE_SUIT_SMALL_BASE + dcf_suit
    var dcf_pipl_tile:  u8 = TILE_PIP_L_BASE + dcf_suit
    var dcf_pipr_tile:  u8 = TILE_PIP_R_BASE + dcf_suit
    var dcf_x1: u8 = dcf_x + 8
    var dcf_y1: u8 = dcf_y + 8
    var dcf_y2: u8 = dcf_y + 16
    // Row 0 — rank corner + small suit
    draw Tileset at: (dcf_x,  dcf_y)  frame: dcf_rank_tile
    draw Tileset at: (dcf_x1, dcf_y)  frame: dcf_small_tile
    // Row 1 — big centre pip (left + right halves)
    draw Tileset at: (dcf_x,  dcf_y1) frame: dcf_pipl_tile
    draw Tileset at: (dcf_x1, dcf_y1) frame: dcf_pipr_tile
    // Row 2 — blank bottom so the card body is symmetric
    draw Tileset at: (dcf_x,  dcf_y2) frame: TILE_FRAME_BLANK_L
    draw Tileset at: (dcf_x1, dcf_y2) frame: TILE_FRAME_BLANK_R
}

// Draw the card-back lattice at (x, y). 6 sprites again. Rows
// 0 and 2 reuse the same top/bottom back tiles; row 1 uses the
// bottom row of the lattice as a filler so the pattern stays
// continuous.
fun draw_card_back(x: u8, y: u8) {
    var dcb_x1: u8 = x + 8
    var dcb_y1: u8 = y + 8
    var dcb_y2: u8 = y + 16
    draw Tileset at: (x,      y)      frame: TILE_BACK_TL
    draw Tileset at: (dcb_x1, y)      frame: TILE_BACK_TR
    draw Tileset at: (x,      dcb_y1) frame: TILE_BACK_BL
    draw Tileset at: (dcb_x1, dcb_y1) frame: TILE_BACK_BR
    draw Tileset at: (x,      dcb_y2) frame: TILE_BACK_TL
    draw Tileset at: (dcb_x1, dcb_y2) frame: TILE_BACK_TR
}

// Dispatch between front and back based on the fly_face_up flag
// stamped by the phase handlers. Snapshots params first because
// the inner call clobbers $04/$05.
fun draw_flying_card(x: u8, y: u8) {
    var dfc_x: u8 = x
    var dfc_y: u8 = y
    if fly_face_up == 0 {
        draw_card_back(dfc_x, dfc_y)
    } else {
        draw_card_face(dfc_x, dfc_y, fly_card)
    }
}

// ── Digits + text ─────────────────────────────────────────

// Draw a single decimal digit 0..9 at (x, y).
fun draw_digit(x: u8, y: u8, d: u8) {
    draw Tileset at: (x, y) frame: TILE_DIGIT_BASE + d
}

// Draw a two-digit decimal count (0..99) at (x, y). Leading zero
// is preserved so the HUD always renders two glyphs wide, which
// keeps the layout stable as the counts change.
fun draw_count(x: u8, y: u8, v: u8) {
    var dct_tens: u8 = 0
    var dct_n: u8 = v
    // Divide by 10 without the software divide: repeatedly
    // subtract 10 until n < 10. For v ≤ 52 this loops at most
    // 5 times, cheaper than calling `/` and `%`.
    while dct_n >= 10 {
        dct_n -= 10
        dct_tens += 1
    }
    draw_digit(x,     y, dct_tens)
    draw_digit(x + 8, y, dct_n)
}

// Draw a single letter 'A'..'Z' using the sprite font. `ch` is
// the letter index (0 = A, 25 = Z). Deliberately NOT marked
// `inline`: when this was inlined the resulting code put each
// inlined `draw` at double the intended X step (the inliner
// appears to re-evaluate the (x + N) parameter expression in a
// way that compounds across consecutive draws). Keeping it as
// a real function call gives every draw_letter call its own
// argument-evaluation context and the spacing comes out right.
fun draw_letter(x: u8, y: u8, ch: u8) {
    draw Tileset at: (x, y) frame: TILE_LETTER_BASE + ch
}

// Short helper for drawing the word "PLAYER" at (x, y). 6 letters.
// Used by the HUD and the victory banner.
//
// We accumulate the X position in a local instead of passing
// `x + N` as a call argument: the latter pattern miscompiles in
// NEScript v0.1 (consecutive `x + N` arguments to the same
// function appear to alias the parameter slot, leaving the second
// onward call site reading a stale offset). Stepping a local
// avoids the issue entirely.
fun draw_word_player(x: u8, y: u8) {
    var dwp_px: u8 = x
    draw_letter(dwp_px, y, 15)  // P
    dwp_px += 8
    draw_letter(dwp_px, y, 11)  // L
    dwp_px += 8
    draw_letter(dwp_px, y, 0)   // A
    dwp_px += 8
    draw_letter(dwp_px, y, 24)  // Y
    dwp_px += 8
    draw_letter(dwp_px, y, 4)   // E
    dwp_px += 8
    draw_letter(dwp_px, y, 17)  // R
}

// "WINS" — used on the victory screen.
fun draw_word_wins(x: u8, y: u8) {
    var dww_px: u8 = x
    draw_letter(dww_px, y, 22)  // W
    dww_px += 8
    draw_letter(dww_px, y, 8)   // I
    dww_px += 8
    draw_letter(dww_px, y, 13)  // N
    dww_px += 8
    draw_letter(dww_px, y, 18)  // S
}

// "PRESS" — used on the title screen.
fun draw_word_press(x: u8, y: u8) {
    var dwr_px: u8 = x
    draw_letter(dwr_px, y, 15)  // P
    dwr_px += 8
    draw_letter(dwr_px, y, 17)  // R
    dwr_px += 8
    draw_letter(dwr_px, y, 4)   // E
    dwr_px += 8
    draw_letter(dwr_px, y, 18)  // S
    dwr_px += 8
    draw_letter(dwr_px, y, 18)  // S
}

// ── Fly animation driver ──────────────────────────────────
//
// We avoid the software multiply (and the W0101 "expensive
// multiply" warning) by stepping the fly position by a fixed
// constant each frame instead of computing `start + dx * t /
// FRAMES`. The constant FLY_STEP is chosen so that
// FRAMES_FLY * FLY_STEP equals the deck-to-play distance on
// both axes:
//
//     FRAMES_FLY * FLY_STEP = 64 px   (16 * 4)
//
// The screen layout (DECK_*_X, PLAY_*_X, DECK_Y, PLAY_Y) is
// arranged so every animation traverses exactly 64 px on both
// axes, which keeps the per-frame step uniform.
const FLY_STEP: u8 = 4

// Step the global (fly_x, fly_y) by FLY_STEP in the directions
// stored in fly_dx_sign / fly_dy_sign. Sign 0 = positive (move
// right / down), 1 = negative (move left / up). Called once per
// frame inside the relevant fly-phase handler.
fun step_fly_pos() {
    if fly_dx_sign == 0 {
        fly_x += FLY_STEP
    } else {
        fly_x -= FLY_STEP
    }
    if fly_dy_sign == 0 {
        fly_y += FLY_STEP
    } else {
        fly_y -= FLY_STEP
    }
}

// Initialise the fly state for a card slide from (sx, sy) using
// the given direction signs. NEScript v0.1 only allocates four
// zero-page slots ($04-$07) for function parameters, so callers
// must stash `fly_card` and `fly_face_up` directly into the
// globals before invoking arm_fly — passing them as the 5th and
// 6th params would silently drop the values on the floor.
fun arm_fly(sx: u8, sy: u8, dxsign: u8, dysign: u8) {
    fly_x = sx
    fly_y = sy
    fly_dx_sign = dxsign
    fly_dy_sign = dysign
}
