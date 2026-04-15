// war/deck.ne — queue operations on the three u8[52] buffers.
//
// Each of deck_a, deck_b, and pot is a 52-entry circular buffer
// storing packed rank/suit bytes. The front index is the next
// card to draw; the count is the number of cards currently in
// the buffer. Everything wraps mod 52 — since 52 isn't a power
// of two, we use `if idx >= 52 { idx -= 52 }` instead of `%` to
// avoid the expensive software mod routine.
//
// NEScript v0.1 has a flat global symbol table for every `var`
// declaration (function-locals included), so each function's
// locals are prefixed with the function's short name to avoid
// E0501 collisions across the program. Function parameters ARE
// scoped per-function, so we can keep their names short.

// ── Helpers shared by every deck ──────────────────────────
//
// Wrap a (front + count) sum back into the 0..51 range. Only
// needed inside push_back because `front` and `count` are both
// ≤ 51 by construction, so the sum is at most 102 and one
// subtraction is enough.
//
// Every parameter name in this file is unique across the entire
// program. NEScript v0.1's IR lowering uses a single global
// var_map for parameter names, so two functions both named
// `wrap52(v: u8)` and `another(v: u8)` end up sharing a VarId
// and the codegen routes their parameter reads to whichever
// zero-page slot the LAST function to be lowered claimed. See
// COMPILER_BUGS.md §1b for the gory details.
inline fun wrap52(w52_v: u8) -> u8 {
    if w52_v >= DECK_SIZE {
        return w52_v - DECK_SIZE
    }
    return w52_v
}

// ── deck_a ────────────────────────────────────────────────

fun deck_a_empty() -> u8 {
    if deck_a_count == 0 {
        return 1
    }
    return 0
}

fun draw_front_a() -> u8 {
    var dfa_card: u8 = deck_a[deck_a_front]
    deck_a_front = wrap52(deck_a_front + 1)
    deck_a_count -= 1
    return dfa_card
}

fun push_back_a(pba_in: u8) {
    // Snapshot `pba_in` into a local before calling wrap52,
    // because NEScript v0.1's parameter-passing ABI uses fixed
    // zero-page slots — wrap52's first param shares slot $04
    // with our `pba_in` and would silently clobber it.
    var pba_card: u8 = pba_in
    var pba_slot: u8 = wrap52(deck_a_front + deck_a_count)
    deck_a[pba_slot] = pba_card
    deck_a_count += 1
}

// ── deck_b ────────────────────────────────────────────────

fun deck_b_empty() -> u8 {
    if deck_b_count == 0 {
        return 1
    }
    return 0
}

fun draw_front_b() -> u8 {
    var dfb_card: u8 = deck_b[deck_b_front]
    deck_b_front = wrap52(deck_b_front + 1)
    deck_b_count -= 1
    return dfb_card
}

fun push_back_b(pbb_in: u8) {
    var pbb_card: u8 = pbb_in
    var pbb_slot: u8 = wrap52(deck_b_front + deck_b_count)
    deck_b[pbb_slot] = pbb_card
    deck_b_count += 1
}

// ── pot ───────────────────────────────────────────────────

fun push_back_pot(pbp_in: u8) {
    pot[pot_count] = pbp_in
    pot_count += 1
}

fun clear_pot() {
    pot_count = 0
}

// Transfer every card currently in the pot into deck_a, in FIFO
// order (so the face-up and face-down cards layer naturally).
fun pot_to_a() {
    var pta_i: u8 = 0
    while pta_i < pot_count {
        push_back_a(pot[pta_i])
        pta_i += 1
    }
    pot_count = 0
}

fun pot_to_b() {
    var ptb_i: u8 = 0
    while ptb_i < pot_count {
        push_back_b(pot[ptb_i])
        ptb_i += 1
    }
    pot_count = 0
}

// ── Init + shuffle ────────────────────────────────────────
//
// Build a 52-card "master deck" in deck_a's backing array using
// a rank-major loop: for each rank 1..13, each suit 0..3, write
// the packed byte (rank << 4) | suit into deck_a[i]. Then
// bounded-random-swap-shuffle it. Finally, split the first 26
// into deck_b (copied), leaving the second 26 in deck_a's first
// half, and reset both queues' cursors.
//
// The random-swap shuffle is a bounded alternative to
// Fisher-Yates: it does N swaps between two random indices, where
// each index is rand() & 0x3F (0..63) and the swap is only done
// when both indices are < 52. 200 iterations on a 52-card deck is
// empirically well-mixed and uses only bitwise ops (no multiply,
// no divide, no W0101 warning).

fun build_master_deck() {
    var bmd_r: u8 = 1
    var bmd_i: u8 = 0
    while bmd_r <= RANK_KING {
        var bmd_s: u8 = 0
        while bmd_s < 4 {
            // Pack rank into the high nibble, suit into the low.
            // rank fits in 4 bits (max 13) and suit fits in 2
            // bits, so the shift-and-or is exact.
            var bmd_shifted: u8 = bmd_r << 4
            deck_a[bmd_i] = bmd_shifted | bmd_s
            bmd_i += 1
            bmd_s += 1
        }
        bmd_r += 1
    }
}

fun shuffle_deck_a() {
    var shf_k: u8 = 0
    while shf_k < 200 {
        var shf_i: u8 = rand_u8() & 0x3F
        var shf_j: u8 = rand_u8() & 0x3F
        if shf_i < DECK_SIZE {
            if shf_j < DECK_SIZE {
                var shf_tmp: u8 = deck_a[shf_i]
                deck_a[shf_i] = deck_a[shf_j]
                deck_a[shf_j] = shf_tmp
            }
        }
        shf_k += 1
    }
}

// After shuffle, split deck_a's 52 cards into two halves: the
// first 26 stay in deck_a, the second 26 move into deck_b.
// Reset both queues' front/count cursors in the process.
fun split_decks() {
    var spd_i: u8 = 0
    while spd_i < HALF_DECK {
        deck_b[spd_i] = deck_a[HALF_DECK + spd_i]
        spd_i += 1
    }
    deck_a_front = 0
    deck_a_count = HALF_DECK
    deck_b_front = 0
    deck_b_count = HALF_DECK
    pot_count = 0
}

fun init_and_shuffle_decks() {
    build_master_deck()
    shuffle_deck_a()
    split_decks()
}
