# Future Work

This document tracks the gaps between what NEScript currently compiles and
what the spec describes. Items are grouped by area. Anything implemented and
tested is omitted — `git log` is the authoritative record of what shipped.

---

## PNG-sourced assets

**What ships today.** `palette Name { colors: [...] }` and
`background Name { tiles: [...], attributes: [...] }` declarations with
inline byte arrays, plus `palette Name @palette("file.png")` and
`background Name @nametable("file.png")` for PNG-sourced variants.
The palette path maps each pixel to its nearest NES master-palette index
(via `nearest_nes_color()` in `src/assets/palette.rs`), deduplicates, and
emits the 32-byte blob; the nametable path slices a 256×240 PNG into the
32×30 tile grid, deduplicates (max 256 unique tiles), and emits the 960+64
byte nametable/attribute blobs. The nametable path now **also auto-generates
the per-tile CHR data** via `png_to_nametable_with_chr` and slots it into
CHR ROM after the user's sprite tile range — see
`examples/auto_chr_background.ne` for the end-to-end flow.
`--memory-map` reports per-blob PRG ROM addresses and a running total
alongside the variable layout.

**Still TODO.**
- **Per-state background rendering control** — programs currently load a
  single nametable at reset. Per-state swaps work but are limited by the
  NMI-time write budget (~2273 cycles, enough for a palette but not a
  full 1024-byte nametable).
- **Per-quadrant palette selection from PNG sources** — the
  `png_to_nametable_with_chr` attribute path picks sub-palettes based on
  brightness buckets, which is fine for grayscale demos but doesn't let
  the user say "this 32×32 tile uses sub-palette 2". A separate
  `palette_map:` shortcut exists for inline backgrounds; the PNG path
  could grow a sibling `@palette_map("hint.png")` that overrides the
  brightness buckets.

---

## User code distribution across switchable banks

**What ships today.** `bank Foo { fun bar() { ... } }` nesting places user
functions into a specific switchable bank. The codegen emits per-bank
instruction streams; the linker runs a two-pass assembly (discover labels
per-bank, then resolve with the merged label table) so banked code can
still reference fixed-bank symbols. Cross-bank calls — both fixed → banked
*and* banked → banked — are rewritten to `JSR __tramp_<name>`, where each
trampoline is a per-function stub in the fixed bank that reads the
caller's current bank from `ZP_BANK_CURRENT`, pushes it on the hardware
stack, switches to the target, JSRs the entry, then pulls and restores
the caller's bank. `gen_mapper_init` seeds `ZP_BANK_CURRENT` with the
fixed bank index at reset so the first cross-bank call from the fixed
bank still leaves the fixed bank mapped at $8000. See
`examples/uxrom_user_banked.ne` (fixed → banked) and
`examples/uxrom_banked_to_banked.ne` (banked → banked).

**Still TODO.**
- **Greedy size-packing.** Placement is explicit-only today — there is no
  pass that takes a program with too much fixed-bank code and
  automatically spills the biggest leaf functions to declared empty banks.
- **MMC3 per-state-handler split** — the `mmc3_per_state_split.ne`
  example still uses the legacy fixed-bank placement for its handlers.
  Extending the banked-fun syntax to state handlers (plus trampoline
  emission on handler dispatch) would unify the two paths. The blocker
  isn't the trampoline — those work for any caller now — but the
  state-handler dispatcher in the IR codegen needs to learn that
  state handlers can live in a switchable bank, and to JSR through a
  trampoline whose entry is the handler label.

---

## Language feature gaps (post-v0.1)

From the spec's "Reserved for Future Versions" section:

| Feature            | Description                                                           |
|--------------------|-----------------------------------------------------------------------|
| **Fixed-point**    | `fixed8.8` type for sub-pixel movement with operator support.         |
| **Text / HUD**     | Font sheet declarations + layout system for scores, health, menus.   |
| **Tilemaps**       | Declarative level data with built-in collision queries.              |
| **SRAM / saves**   | Persistent storage declarations for battery-backed save data.        |

NES 2.0 headers are now supported via `game Foo { header: nes2 }` — see
`src/rom/mod.rs`.

**Metasprites** are now supported via `metasprite Name { sprite: ...,
dx: [...], dy: [...], frame: [...] }` — see `examples/metasprite_demo.ne`.
The IR lowering expands `draw Hero at: (x, y)` into one `DrawSprite` op
per tile, with each tile's frame index offset by the underlying sprite's
base tile so the codegen sees a stream of regular draws and the OAM
cursor allocator picks them up unchanged. Negative offsets and
runtime-varying tile selection are still TODO — the current form takes
literal `u8` offsets.

### Struct / array field widths

Nested struct fields (`hero.pos.x`) and array struct fields
(`hero.inv[i]`) now compile end-to-end. The analyzer recursively
flattens the struct layout into per-leaf synthetic variables (with
intermediate `Struct(...)` symbols for the dotted prefixes), and the
parser loops the dotted chain in `parse_primary` and
`parse_assign_or_call` so the existing `format!("{name}.{field}")`
synthetic-name model still works without IR changes. Array-of-structs
is still rejected with E0201 — the synthetic-variable model can't
index per-element struct layouts without further codegen work, see
`src/analyzer/mod.rs::register_struct`.

---

## Audio pipeline

**What ships today.** Frame-walking pulse driver with `sfx Name { duty, pitch,
volume }` and `music Name { duty, volume, repeat, notes }` blocks; builtin
effects and tracks; a 60-entry period table; `__audio_used` marker that
elides the whole subsystem when no program statement references it. **Plus**
`channel: triangle` and `channel: noise` on `sfx` blocks, which splice in
per-channel slots that write to $4008-$400B (triangle) or $400C-$400F
(noise) when a program declares them. **Plus per-frame pitch envelopes
on Pulse-1 sfx** — a `pitch:` array with more than one distinct value
opts into a separate `__sfx_pitch_<name>` blob that the audio tick walks
in lockstep with the volume envelope, writing `$4002` on every NMI for a
real frequency-sweeping pulse channel. Pulse-only programs without
varying-pitch sfx still produce byte-identical driver code. See
`examples/noise_triangle_sfx.ne` and `examples/sfx_pitch_envelope.ne`.

**Still TODO for richer audio.**
- **DMC channel** — delta-modulation sample playback is not wired yet.
- **Multi-channel tracker playback** — one `notes` list per channel on
  `music` blocks (the triangle/noise SFX are one-shot envelopes, not a
  tracker).
- **`@sfx("file.nsf")` / `@music("file.ftm")`** — neither the NSF nor the
  FamiTracker format is parsed yet.
- **Per-frame pitch envelopes on triangle / noise sfx** — the data
  shape (a parallel pitch array on the `sfx` block) is the same as for
  Pulse-1, but the runtime triangle/noise tick blocks currently only
  write their volume registers (`$4008` / `$400C`). Extending them to
  also walk a per-channel pitch envelope and write `$400A` / `$400E`
  is the natural next step now that the pulse path is proven.

---

## Debug instrumentation

**What ships today.** `debug.log(...)` and `debug.assert(...)` lower to $4800
writes when `--debug` is passed, and are stripped entirely in release builds.
`--symbols <path>` writes a Mesen-compatible `.mlb` file listing function,
state-handler, and variable addresses (with PRG ROM offsets for code and
CPU addresses for RAM). `--source-map <path>` consumes the `SourceLoc` IR
op and writes a plain-text map of `<rom_offset> <file_id> <line> <col>`
entries for every lowered statement. Debug builds emit array bounds checks
(CMP against size, BCC past a `JMP __debug_halt` wedge) and bump an
overrun counter at `$07FF` in the NMI handler when the main loop didn't
reach `wait_frame` before the next vblank.

**Plus four query expressions** that mirror the counter/sticky pattern:
`debug.frame_overrun_count()` / `debug.frame_overran()` return the cumulative
overrun counter and a per-frame sticky bit so user code can write
`debug.assert(not debug.frame_overran())` guards, and
`debug.sprite_overflow_count()` / `debug.sprite_overflow()` do the same for
the NES PPU's sprite-per-scanline flag (`$2002` bit 5), which the NMI
handler samples once per frame in debug mode. All four sticky bits clear
on the next `wait_frame`.

**Still TODO.**
- **`debug.overlay(x, y, text)`** — needs the text/HUD subsystem (see
  Language feature gaps).

---

## Code quality / tooling

### Register allocator

All IR temps currently spill to a recycled zero-page slot (`$80-$FF`). The
peephole pass mops up the most obvious waste, but a real CFG-aware allocator
that holds short-lived temps in `A`/`X`/`Y` would cut a noticeable number of
LDA/STA pairs.

### Cross-block temp live-range analysis

The slot recycler is function-local per-block. Temps that flow across block
boundaries get a dedicated slot for the entire function, even if a later
block could reuse the slot.

### WASM build target

To build a browser IDE we would need to route file I/O through a trait so the
core pipeline works on `&str → Vec<u8>` without touching `std::fs`. Today the
parser's preprocess pass and the asset resolver read files directly.

---

## Error message polish

### Unused error codes

`ErrorCode` only defines codes that are actually emitted. Previously there
were placeholder variants (`E0202` invalid cast, `E0403` unreachable state)
marked `#[allow(dead_code)]`; those were removed during cleanup. If those
semantics come back, add the codes at that point.

---

## Open design questions

1. **Inline asm label syntax.** `.label:` (ca65 style) vs `label:` (generic)?
   Today the inline-asm parser accepts `label:` but not `.label`; migrating
   would be cheap but would invalidate any copy-pasted ca65 fragments.
2. **Debug port address.** $4800 is conventional but not universal. Should
   we support multiple debug output methods?
3. **OAM allocation strategy.** Sequential allocation remains the default;
   the `cycle_sprites` opt-in keyword rotates the DMA offset each frame so
   scenes past the 8-per-scanline budget flicker instead of dropping the
   same sprite every frame. Open question: should automatic cycling become
   a `game` attribute (`sprite_flicker: true`) that emits the increment
   without requiring a per-frame call, and/or add a `draw ... priority:
   pinned` modifier for HUD sprites that must stay at low OAM slots?
4. **Error recovery granularity.** How aggressively should the parser
   recover? More recovery means more errors per compile but also risks
   cascading false errors.
