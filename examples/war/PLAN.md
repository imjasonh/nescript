# War — Implementation Plan

A production-quality card game example for NEScript. This document is
the living plan: each step is checked off as it's completed, and any
mid-flight design changes are recorded in the "Design revisions" section
at the bottom.

---

## 1. Scope & quality bar

A production-grade NES version of War:

- **Title screen** with a big logo, a 3-option menu (`0 PLAYERS` /
  `1 PLAYER` / `2 PLAYERS`), blinking "PRESS START", and music.
- **Gameplay** with an animated deal, two live decks with running card
  counts, cards that slide between the decks and the center, a readable
  face-up card on each side.
- **War / tie-breaker** flow with a distinct "WAR!" banner, an exciting
  SFX, and buried cards that actually come out of each player's deck.
- **Victory screen** with the winning player highlighted and a fanfare.
- **Sound**: title music, card-flip click, distinct round-win cues for
  A vs B, exciting WAR tie-break sfx, victory fanfare.

---

## 2. File layout

```
examples/war.ne                  # top-level: game decl, palette, background, sprite sheet, includes, start
examples/war/PLAN.md             # this file
examples/war/constants.ne        # gameplay + layout constants
examples/war/audio.ne            # sfx + music declarations
examples/war/state.ne            # global variables (decks, phase, rng, anim timers, ...)
examples/war/rng.ne              # 8-bit LFSR PRNG helpers
examples/war/deck.ne             # queue ops on decks + pot
examples/war/compare.ne          # card rank/suit extraction, round resolver
examples/war/render.ne           # draw_card_front / draw_card_back helpers
examples/war/title_state.ne      # state Title
examples/war/deal_state.ne       # state Deal
examples/war/play_state.ne       # state Playing (phase machine)
examples/war/victory_state.ne    # state Victory
```

The top-level `examples/war.ne` is the file the pre-commit hook and
emulator harness see. It compiles to `examples/war.nes` without any
tooling changes. The split source files all live under `examples/war/`
and are pulled in via `include "war/<name>.ne"` directives.

---

## 3. Hardware budget

### Sprite budget per frame

All sprites use sprite sub-palette 0 (the NEScript codegen hard-wires
the OAM attribute byte to 0). That gives 4 shared colours:
**transparent, red, white, black** — enough for both red and black suits
on a white card face with a black outline.

Cards are **16×24 px (2 cols × 3 rows = 6 sprites)**. Max on-screen
simultaneously in the steady state:

| Entity        | Qty | Sprites | Notes |
|---------------|-----|---------|-------|
| Deck A top    | 1   | 6       | card back |
| Deck B top    | 1   | 6       | card back |
| Face-up A     | 1   | 6       | varies by round |
| Face-up B     | 1   | 6       | varies by round |
| Flying card   | 1   | 6       | only during anim |
| Cursor / UI   | ≤2  | ≤2      | title selector + misc |
|               |     | **32**  | ≤ 64 OAM slots ✓ |

**8-per-scanline check.** Decks are at `y = 80`, face-up cards at
`y = 128`. Those two bands never overlap vertically, so the worst case
is 2 cards side by side within the same band = 4 sprites per scanline.

### Tile budget (max 256)

| Group | Tiles | Purpose |
|-------|-------|---------|
| Card frame | 6 | outline corners + top/bottom blank cells |
| Card back  | 4 | diamond-lattice pattern, symmetric |
| Rank glyphs | 13 | A, 2-9, 10, J, Q, K (8×8 each) |
| Small suit (corner) | 4 | ♠ ♥ ♦ ♣ |
| Big suit left half | 4 | left half of large centre pip |
| Big suit right half | 4 | right half |
| Font A-Z | 26 | title/menu/HUD |
| Font 0-9 | 10 | deck counts + HUD |
| Punctuation + space | ~4 | `:`, `!`, space, `?` |
| Big "WAR" banner | 12 | 2×2 block per letter |
| Felt-table tile | 2 | background fill + subtle pattern |
| Border / divider | 4 | thin framing |
| Cursor / arrow | 2 | menu selection |
| **Total**  | **~95** | leaves ~150 tiles free |

### RAM budget

| Structure | Bytes | Notes |
|-----------|-------|-------|
| `deck_a: u8[52]` | 52 | circular buffer, packed `rank<<4 \| suit` |
| `deck_b: u8[52]` | 52 | |
| `pot: u8[52]`    | 52 | cards currently in play |
| queue cursors, counts, phase, anim timers | ~20 | |
| RNG state | 1 | |
| **Total** | **~180** | well under the ~1700 bytes of general RAM |

---

## 4. Card representation & RNG

### Card encoding

One byte per card, packed as `(rank << 4) | suit`:

- `rank` = 1..13 (`A=1`, `2=2`, …, `10=10`, `J=11`, `Q=12`, `K=13`)
- `suit` = 0..3 (`♠=0`, `♥=1`, `♦=2`, `♣=3`)

### PRNG

8-bit Galois LFSR seeded from the frame counter on title-screen exit.

### Shuffle

Bounded random-swap shuffle: 200 swaps between two 6-bit random
indices, retrying when either index ≥ 52. Uses only `&` and compare,
no multiply or divide.

### Deck as a queue

Each deck is a circular buffer with `front` / `count` cursors. `draw`
bumps `front`, `push_back` writes at `(front + count) % 52`. Modulo is
implemented with `if x >= 52 { x -= 52 }` to avoid the expensive `%`
software routine.

---

## 5. State machine

```
Title → Deal → Playing → Victory → Title
```

`Playing` contains an inner phase machine driven by a `u8` phase var:

| Phase | Notes |
|-------|-------|
| `P_WaitA` | Human: wait for input; CPU: wait for think timer |
| `P_FlyA` | 16-frame lerp of A's card from deck to play position |
| `P_WaitB` | symmetric |
| `P_FlyB` | symmetric |
| `P_Reveal` | Both cards visible; brief beat |
| `P_Resolve` | Compare, branch to `P_WinA` / `P_WinB` / `P_WarBanner` |
| `P_WinA` / `P_WinB` | Cards slide to winner deck |
| `P_WarBanner` | Flash "WAR!" banner, play `WarFlash` |
| `P_WarBury` | Bury 3 face-down cards from each deck |
| `P_Check` | Win-condition check, possibly `transition Victory` |

Game modes (`0`/`1`/`2` players) are captured by two `bool` flags
`a_is_cpu` / `b_is_cpu` chosen on title exit.

---

## 6. Audio

| Name       | Channel | Shape |
|------------|---------|-------|
| `FlipCard` | pulse 1 | short descending click |
| `CheerA`   | pulse 1 | 8-frame rising arpeggio, pitch envelope |
| `CheerB`   | pulse 1 | 8-frame descending arpeggio, pitch envelope |
| `WarFlash` | pulse 1 | 16-frame pitch sweep, loud → soft → loud |
| `ThudDown` | noise | 4-frame noise burst (bury animation) |
| `TitleTheme` | pulse 2 | brisk 4/4 march, looping |
| Victory | builtin `fanfare` | one-shot on win |

---

## 7. Implementation steps

The checkboxes below are updated as each step lands. A step is only
marked complete after the compiler still produces a working ROM and
the change visibly does what it's supposed to.

- [ ] Step 1: Skeleton — top-level file, empty included files, compiles.
- [ ] Step 2: Palette, felt background, card-back tiles, static card
      rendered on the table.
- [ ] Step 3: Card-face tiles (ranks, suits, big pips) + draw_card_face
      helper showing all 13 ranks × 4 suits.
- [ ] Step 4: Deck data structures, RNG, shuffle, init + HUD digit
      rendering.
- [ ] Step 5: Title state (WAR banner, 3-option menu, cursor, music,
      autopilot).
- [ ] Step 6: Deal state (animated deal + flip sfx).
- [ ] Step 7: Play state phase machine with card animations and sfx.
- [ ] Step 8: War tie-break (banner + bury animation + WarFlash).
- [ ] Step 9: Victory state (winner banner + fanfare + auto-return).
- [ ] Step 10: Polish pass (timing, readability, sfx/music tuning).
- [ ] Step 11: Rebuild war.nes, capture goldens, verify diff.
- [ ] Step 12: Update README.md and examples/README.md.
- [ ] Code review pass: read every file end-to-end, fix mistakes.

---

## 8. Design revisions

(Empty — will be updated in-flight if anything changes from the
approved design.)
