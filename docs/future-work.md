# Future Work

This document tracks the gaps between what NEScript currently compiles and
what the spec describes. Items are grouped by area. Anything implemented and
tested is omitted — `git log` is the authoritative record of what shipped.

---

## PNG-sourced palette and nametable assets

**What ships today.** `palette Name { colors: [...] }` and
`background Name { tiles: [...], attributes: [...] }` declarations with
inline byte arrays. The first declared palette / background is loaded
at reset time (before rendering enables), and both blocks get named
data blobs in PRG ROM so `set_palette Name` / `load_background Name`
can queue a vblank-safe swap via the NMI handler. See
`examples/palette_and_background.ne`.

**Still TODO.**
- `@palette("file.png")` — analyze image colours and map to nearest
  NES master-palette indices. `nearest_nes_color()` already lives in
  `src/assets/palette.rs` but is not wired through the resolver.
- `@nametable("file.png")` — convert a 256×240 image into a 960-byte
  nametable plus 64-byte attribute table with automatic tile
  deduplication (max 256 unique tiles per pattern table).
- Per-state background rendering control — programs currently get a
  single fixed nametable at reset; per-state swaps work but are
  limited by the NMI-time write budget (~2273 cycles, enough for a
  palette but not a full 1024-byte nametable).
- `--memory-map` should report palette and background PRG ROM usage
  alongside the variable layout.

---

## User code distribution across switchable banks

**Status.** `mapper: MMC1 / UxROM / MMC3` plus top-level `bank Name { prg }`
declarations are honored by the iNES header and by the linker, which reserves
each declared bank as a 16 KB switchable slot. However, the IR codegen puts
every user function and state handler into the fixed bank at `$C000-$FFFF` —
the declared banks exist only as empty space. Programs outgrowing the fixed
16 KB have nowhere to put their code.

**What's needed.**
- A bank-assignment step (analyzer or a new pass) that maps each user function
  / state handler to a target bank, either via explicit `bank Foo { fun bar()
  ... }` nesting or by greedy size-packing.
- Codegen support for emitting into non-fixed banks and for generating
  cross-bank trampolines (the runtime helper scaffold already exists in
  `runtime/mod.rs::gen_bank_trampoline`; it just isn't invoked).
- Linker changes so that functions in a switchable bank are found by the
  JSR fix-up logic when the call crosses bank boundaries.

---

## Language feature gaps (post-v0.1)

From the spec's "Reserved for Future Versions" section:

| Feature            | Description                                                           |
|--------------------|-----------------------------------------------------------------------|
| **Fixed-point**    | `fixed8.8` type for sub-pixel movement with operator support.         |
| **Text / HUD**     | Font sheet declarations + layout system for scores, health, menus.   |
| **Metasprites**    | Multi-tile sprite groups with relative positioning.                   |
| **Tilemaps**       | Declarative level data with built-in collision queries.              |
| **SRAM / saves**   | Persistent storage declarations for battery-backed save data.        |
| **NES 2.0 header** | Extended iNES header format for additional metadata.                  |

### Struct / array field widths

Struct and array element types are currently restricted to the single-byte
primitives (`u8`, `i8`, `bool`). `u16`, nested struct fields, and array fields
are rejected with `E0201`. The analyzer's field-layout machinery needs to
grow multi-byte offsets, and IR lowering needs to treat wide fields as the
existing wide-var path already does for `u16` globals.

---

## Audio pipeline

**What ships today.** Frame-walking pulse driver with `sfx Name { duty, pitch,
volume }` and `music Name { duty, volume, repeat, notes }` blocks; builtin
effects and tracks; a 60-entry period table; `__audio_used` marker that
elides the whole subsystem when no program statement references it.

**Still TODO for richer audio.**
- Triangle / noise / DMC channels (today the driver only uses pulse 1 and
  pulse 2).
- Multi-channel tracker playback (one `notes` list per channel).
- `@sfx("file.nsf")` / `@music("file.ftm")` asset directives — neither the
  NSF nor the FamiTracker format is parsed yet.
- Per-note pitch changes within a sfx (currently `pitch` latches once at
  trigger time).

---

## Debug instrumentation

**What ships today.** `debug.log(...)` and `debug.assert(...)` lower to $4800
writes when `--debug` is passed, and are stripped entirely in release builds.

**Not yet implemented.**
- Mesen-compatible symbol export (`.mlb` / `.sym` files) — the CLI does not
  emit them, and the previous `DebugSymbols` helper was removed as dead code
  during cleanup.
- Source maps relating ROM addresses to source lines — the `SourceLoc`
  IR op exists but is not consumed by the linker or CLI.
- Array bounds checking in debug mode.
- Frame overrun detection (cycles-per-frame counting).
- `debug.overlay(x, y, text)` — needs the text/HUD subsystem above.

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

### `--no-opt` flag

There is no way to disable the optimizer from the CLI. Adding one would make
optimizer-introduced bugs easier to bisect.

### Compilation benchmarks

Compilation is fast (<100 ms for every example today) but has no `cargo
bench` harness, so regressions would slip through.

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

### Missing diagnostics

- No warning for implicit-drop of a function return value (`my_fun()` at
  statement position when `my_fun` returns non-void).
- `W0102` ("loop without yield") is only emitted for bare `loop`, not for
  `while true` or `loop { if cond { continue } }`.
- No warning for `fast` variables that never justify the zero-page slot
  (could cross-reference access counts).
- No warning when a `palette` declaration has inconsistent "index 0" bytes
  across its eight sub-palettes. The NES hardware mirrors `$3F10/$3F14/
  $3F18/$3F1C` onto `$3F00/$3F04/$3F08/$3F0C`, so writing the full 32-byte
  blob sequentially causes the last four "sprite sub-palette 0" bytes to
  overwrite the background universal colour; the fix is a user-side
  convention (every sub-palette's first byte equals the chosen universal
  colour) but the analyzer doesn't warn when a declaration violates it.
  The mistake produced a solid-black screen in `examples/platformer.ne`
  until it was chased down by hand.

---

## Open design questions

1. **Inline asm label syntax.** `.label:` (ca65 style) vs `label:` (generic)?
   Today the inline-asm parser accepts `label:` but not `.label`; migrating
   would be cheap but would invalidate any copy-pasted ca65 fragments.
2. **Debug port address.** $4800 is conventional but not universal. Should
   we support multiple debug output methods?
3. **OAM allocation strategy.** Sequential allocation vs priority-based with
   automatic sprite cycling for the 8-per-scanline limit?
4. **Error recovery granularity.** How aggressively should the parser
   recover? More recovery means more errors per compile but also risks
   cascading false errors.
