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
byte nametable/attribute blobs. `--memory-map` reports per-blob PRG ROM
addresses and a running total alongside the variable layout.

**Still TODO.**
- **Automatic CHR generation from `@nametable` PNGs** — the nametable
  resolver currently produces tile indices 0..N but does not write matching
  CHR data, so users still need to supply CHR via `@chr(...)` with the same
  tile ordering. Closing this gap requires coordinating the PNG tile
  dedupe with the CHR allocator so both pipelines agree on indices.
- **Per-state background rendering control** — programs currently load a
  single nametable at reset. Per-state swaps work but are limited by the
  NMI-time write budget (~2273 cycles, enough for a palette but not a
  full 1024-byte nametable).

---

## User code distribution across switchable banks

**What ships today.** `bank Foo { fun bar() { ... } }` nesting places user
functions into a specific switchable bank. The codegen emits per-bank
instruction streams; the linker runs a two-pass assembly (discover labels
per-bank, then resolve with the merged label table) so banked code can
still reference fixed-bank symbols. Fixed → banked calls are rewritten to
`JSR __tramp_<name>`, where each trampoline is a per-function stub in the
fixed bank that saves the current bank, switches, calls the target,
restores, and returns. `runtime/mod.rs::gen_bank_trampoline` is the
per-mapper emitter. See `examples/uxrom_user_banked.ne`.

**Still TODO.**
- **Banked → banked cross-bank calls.** The codegen panics if a function
  in bank A tries to call a function in bank B. The fix is to generalize
  the trampoline registry so the caller's bank restore logic works for
  arbitrary target banks, not just calls originating in the fixed bank.
- **Greedy size-packing.** Placement is explicit-only today — there is no
  pass that takes a program with too much fixed-bank code and
  automatically spills the biggest leaf functions to declared empty banks.
- **MMC3 per-state-handler split** — the `mmc3_per_state_split.ne`
  example still uses the legacy fixed-bank placement for its handlers.
  Extending the banked-fun syntax to state handlers (plus trampoline
  emission on handler dispatch) would unify the two paths.

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

NES 2.0 headers are now supported via `game Foo { header: nes2 }` — see
`src/rom/mod.rs`.

### Struct / array field widths

`u16` struct fields now compile. Nested struct fields and array fields are
still rejected with `E0201`; the field-layout accumulator handles variable
sizes correctly, but the IR-lowering side needs extending to recurse for
nested structs and to multiply by element size for array fields.

---

## Audio pipeline

**What ships today.** Frame-walking pulse driver with `sfx Name { duty, pitch,
volume }` and `music Name { duty, volume, repeat, notes }` blocks; builtin
effects and tracks; a 60-entry period table; `__audio_used` marker that
elides the whole subsystem when no program statement references it. **Plus**
`channel: triangle` and `channel: noise` on `sfx` blocks, which splice in
per-channel slots that write to $4008-$400B (triangle) or $400C-$400F
(noise) when a program declares them. Pulse-only programs still produce
byte-identical driver code. See `examples/noise_triangle_sfx.ne`.

**Still TODO for richer audio.**
- **DMC channel** — delta-modulation sample playback is not wired yet.
- **Multi-channel tracker playback** — one `notes` list per channel on
  `music` blocks (the triangle/noise SFX are one-shot envelopes, not a
  tracker).
- **`@sfx("file.nsf")` / `@music("file.ftm")`** — neither the NSF nor the
  FamiTracker format is parsed yet.
- **Per-note pitch changes within a sfx** — `pitch` latches once at
  trigger time.

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

**Still TODO.**
- **`debug.overlay(x, y, text)`** — needs the text/HUD subsystem (see
  Language feature gaps).
- **Richer frame overrun telemetry** — today a single counter is bumped.
  A `debug.frame_overrun_count()` builtin that exposes the counter to user
  code, plus a per-frame "did this frame overrun" bit for
  `debug.assert!(no_overrun)`-style guards, would make the data more
  actionable.

---

## Decompilation support

**What ships today.** A hybrid-shim decompiler that converts iNES ROMs back to NEScript `.ne` source code, enabling round-trip workflows (ROM → decompile → edit → recompile). The implementation is structured around NEScript-produced ROMs, which can be byte-identically reconstructed from decompiled source.

### Completed (M1-M5)

**Language foundations for decompilation (M1):**
- `fixed8.8` type support added to `NesType` enum (partial: type system only; arithmetic codegen pending)
- AST infrastructure for address-pinned declarations (fields added to VarDecl/ConstDecl/PaletteDecl/BackgroundDecl/SfxDecl/MusicDecl; parser integration deferred)

**Decompiler infrastructure (M3):**
- `src/decompiler/` module: `mod.rs` (main pipeline), `lifter.rs` (asset extraction), `emitter.rs` (.ne source generation), `patterns/` (driver recognition)
- `decompile_rom()` function: reads iNES ROM, validates header, extracts ROM metadata, banks, CHR data
- `DecompiledRom` struct holding: rom metadata, mapper, PRG/CHR banks, extracted palettes/backgrounds (stubs for M3)
- Mapper detection (iNES number → NEScript Mapper enum)

**FamiTone2 driver recognition (M4):**
- `patterns/famitone.rs`: Detects FamiTone2 driver via period table pattern matching (60-entry table, APU range validation)
- `patterns/audio.rs`: Period table inversion (APU period value ↔ note index C1-B5), note lookup map
- Audio data extraction stubs ready for full music/SFX blob parsing
- Period table correctly validates all 60 entries and table monotonicity

**Round-trip integration tests (M5):**
- `tests/decompiler_roundtrip.rs`: Test harness for identity roundtrip (decompile → recompile → byte-compare)
- `roundtrip_identity_all_examples`: Decompiles all 22 examples, recompiles, verifies ROM byte-identical (currently #[ignore] pending M1 full language support)
- `roundtrip_emulator_all_examples`: Decompiles, recompiles, prepares for jsnes emulator golden testing (currently #[ignore] pending M4 audio data extraction)
- Smoke tests verify all examples are accessible and decompiler infrastructure operational
- CI job `decompile-roundtrip` in `.github/workflows/ci.yml` with artifact upload on failure

### Still TODO (next phase)

**Language foundations (M1 completion):**
- Parser/analyzer support for @ 0xADDR address-pinned syntax on all declaration types
- raw_bank pass-through: parser recognition and linker handling for binary file splice
- raw_vectors opt-out: custom reset/nmi/irq addresses instead of auto-generated
- free_space regions for relocation of oversized assets
- fixed8.8 arithmetic codegen: +, -, *, /, comparison operations

**goto/label escape hatch (M2):**
- goto/label statement support with W0109 diagnostic for unstructured code without #[allow(unstructured)]
- Lexer support for # attribute prefix to enable #[allow(unstructured)]

**Decompiler CLI integration:**
- `nescript decompile <rom.nes> [-o output.ne]` subcommand wiring in main.rs

**Asset lifting refinement:**
- Full CHR → PNG conversion (or binary pass-through)
- Palette blob recognition (32-byte sections with color indices)
- Nametable blob recognition (960+64 byte sections)
- AST infrastructure for address-pinned palette/background/sfx/music declarations

**Audio data extraction (M4 completion):**
- Music block parsing (note sequences with duration)
- SFX block parsing (pitch/volume envelopes)
- Structured music/sfx declaration emission instead of binary blobs

**Decompiler scope for non-NEScript ROMs:**
- Fingerprinting strategy for unknown/third-party ROMs (conservative identity pass-through)
- Heuristic-based audio driver detection (FamiTone2, Nintendo standard, custom)

### Decompilation design principles

The decompiler uses a **hybrid-shim approach** rather than full structured lift:
- **Raw pass-through:** Opaque PRG/CHR code banks pass through verbatim as `raw_bank` declarations; no 6502 decompilation
- **Lifted assets:** Palettes, nametables, sprite CHR, audio drivers → structured .ne declarations
- **Address-pinned declarations:** Decompiled source pins every asset to its original ROM byte offset, so edits recompile in-place or relocate within free space
- **Round-trip oracle:** Emulator golden harness (existing pixel/audio hash tests) is the correctness bar, not binary ROM match

This avoids building a full-strength 6502 decompiler (years of work, lower ROI) while enabling the practical workflow: decompile an existing game, edit constants/audio/sprites, recompile.

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
3. **OAM allocation strategy.** Sequential allocation vs priority-based with
   automatic sprite cycling for the 8-per-scanline limit?
4. **Error recovery granularity.** How aggressively should the parser
   recover? More recovery means more errors per compile but also risks
   cascading false errors.
