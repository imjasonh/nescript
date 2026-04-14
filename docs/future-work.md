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

## ROM decompilation and hybrid-shim mods

**Motivation.** The hand-rolled asm disassemblies that currently drive NES ROM
hacking (SMB, MM2, Zelda) let modders edit physics, sounds, palettes, and
levels without touching raw bytes, but the asm form is unreadable for anyone
who doesn't already know 6502. We want a workflow where someone can load an
existing ROM, edit it as `.ne` source (change sound effects, tweak player
physics, swap palettes, replace music, bump starting lives), and rebuild a
working ROM. The goal is **not** a full lift of arbitrary 6502 to idiomatic
structured `.ne` — that's unbounded work and the output would be unreadable
anyway. The goal is a **hybrid shim**: most of the original ROM passes
through as verbatim PRG/CHR bytes, and the interesting, editable parts
(assets, constants, data tables) are lifted to structured `.ne` declarations
pinned at their original addresses.

**Design.** A decompiled program looks like:

```
game "MegaMan 2" { mapper: MMC1, header: nes2 }

// Original code passes through unchanged. The reset/NMI/IRQ vectors
// point at the original ROM's handlers, not the runtime we would
// normally splice in.
raw_bank Bank0 @ 0 { binary: "mm2.bank0.bin" }
raw_bank Bank1 @ 1 { binary: "mm2.bank1.bin" }
// ...
raw_vectors { reset: 0xC123, nmi: 0xC000, irq: 0xFFB4 }

// CHR passes through as PNGs so it's editable in an image editor.
chr_bank Tiles0 @ 0 { source: "mm2.chr0.png" }

// Assets the decompiler recognized by pattern-matching the
// standard driver formats — these are pinned at their original
// addresses and recompile back into the same byte ranges.
palette StageTitle @ 0x1E400 { universal: 0x0F, bg0: [...], ... }
music   StageIntro @ 0x1E800 { pulse1: [...], pulse2: [...], tri: [...], noise: [...] }
sfx     MegaBuster @ 0x1E980 { duty: 2, envelope: [...] }

// Physics constants the decompiler found by symbolic analysis of the
// player-update routine.
const PLAYER_WALK_SPEED:    fixed8.8 @ 0x13E02 = 1.25
const PLAYER_JUMP_VELOCITY: fixed8.8 @ 0x13E04 = -4.75
const STARTING_LIVES:       u8       @ 0x13A00 = 2
```

The user edits the lifted declarations, the compiler rebuilds them into
byte blobs, and the linker overwrites those specific byte ranges in the
pass-through banks. Everything the decompiler couldn't identify stays as
opaque raw bytes — which is exactly how much effort they deserve.

The rest of this section is the concrete work items that path needs.
Items already tracked elsewhere in this doc (fixed-point, metasprites,
multi-channel tracker music, DMC, banked→banked calls) are cross-referenced
rather than duplicated.

### Address-pinned declarations

Today `Placement` in `src/parser/ast.rs` is `Fast` / `Slow` / `Auto`. The
compiler picks the final address. For the decompiler we need the author
(the decompiler itself, or a user editing its output) to say **exactly**
where a declaration lives:

- `var lives: u8 @ 0x075A` — pin a RAM variable at a known address so
  game code that already reads `$075A` continues to work.
- `const WALK_SPEED: fixed8.8 @ 0x13E02 = 1.25` — pin a ROM-resident
  constant inside a `raw_bank`, replacing the two bytes at that offset.
- `palette StageTitle @ 0x1E400 { ... }`, `music Intro @ 0x1E800 { ... }`,
  `sfx Jump @ 0x1E980 { ... }` — pin asset blobs at a specific PRG offset.

Work:

- Extend `Placement` with `Fixed(u16)` (RAM) and `PrgOffset(u32)` (ROM).
- Parser grammar additions for `@ 0xADDR` after the type on `var` / `const`
  declarations and after the name on asset blocks.
- Analyzer: validate that pinned RAM addresses don't collide with runtime
  reservations (`$00-$0F`, `$11-$17`, `$80-$FF` temps), or suppress those
  reservations entirely when `raw_vectors` is in use.
- Linker: write each pinned-offset asset into the passed-through bank
  bytes at the exact offset, failing if the recompiled blob is larger
  than the original.
- Diagnostics: a new error code for "pinned blob overflows original
  slot by N bytes," with a help pointer to the free-space declaration
  described below.

### Raw PRG bank pass-through

`linker::PrgBank::with_data` already carries a raw byte payload —
switchable banks constructed via `PrgBank::with_data` get spliced into the
ROM verbatim. What's missing is a language-surface declaration for it.

- `raw_bank Name @ <index> { binary: "file.bin" }` at the top level.
  `<index>` is the physical bank number; `file.bin` is loaded at
  compile time and must be exactly 16 KB (8 KB for NROM half-banks).
- Fixed-bank variant: `raw_bank Fixed @ fixed { binary: "fixed.bin" }`.
  Writing to the fixed bank slot replaces the runtime splice — see
  "Runtime opt-out" below.
- Analyzer: detect overlap between a `raw_bank` and compiled user
  functions placed in the same bank; reject with a diagnostic.
- Asset resolver: load binary files via the same path the existing
  `binary: "…"` inline-asm asset source uses.

### Runtime opt-out and custom vectors

Every `.ne` program today forcibly splices `runtime::gen_init`,
`gen_nmi`, `gen_irq`, `gen_mapper_init`, `gen_audio_tick`, and the mapper
trampolines into the fixed bank, then stamps the iNES reset/NMI/IRQ
vectors to point at them. A decompiled ROM has to keep the **original**
reset/NMI/IRQ handlers in place — the original game knows nothing about
our runtime, OAM cursor convention, frame-ready flag, audio tick, or
state dispatch.

- `raw_vectors { reset: 0xADDR, nmi: 0xADDR, irq: 0xADDR }` at the top
  level. Any of the three can be omitted; omitted vectors fall back to
  the runtime splice as today.
- When `raw_vectors` is present, the linker skips `gen_init` / `gen_nmi`
  / `gen_irq` and writes the named addresses into the 6-byte vector
  table at `$FFFA-$FFFF`.
- When `raw_vectors` is present, the analyzer also suppresses the
  runtime's zero-page reservations (`$00-$0F`, `$11-$17`, `$80-$FF`) so
  user declarations can pin variables anywhere without false-positive
  collision errors. The runtime helpers (`play`, `start_music`, `draw`,
  `set_palette`, etc.) become unavailable in that mode — a program
  using `raw_vectors` that also calls them is rejected with a clear
  "hybrid-shim ROMs must address hardware directly via `peek`/`poke` or
  inline asm" error.
- A less drastic partial opt-out: `runtime { audio: false, oam: false,
  ppu_update: false }` flags on the `game` block, for the transitional
  case where you want *some* NEScript conveniences but a custom NMI.
  Probably a follow-up, not day-one.

### Raw and PNG-sourced CHR banks

`chr_bank Name { source: "file.png" }` as a first-class top-level
declaration — right now sprites and backgrounds are the only path to
CHR ROM, and they're both tightly coupled to the sprite/nametable
resolver. A decompiled ROM needs to dump the original CHR verbatim
(typically as 8 KB per bank), swap it for an edited PNG without having
to restructure it into `sprite` / `background` blocks.

- `chr_bank Name @ <index> { binary: "file.chr" }` — verbatim 8 KB
  blob at the given CHR bank slot.
- `chr_bank Name @ <index> { source: "file.png", tiles_wide: 16,
  tiles_tall: 16 }` — convert a PNG into a 256-tile CHR blob using
  the existing `src/assets/chr.rs` pipeline, but without requiring a
  surrounding `sprite` declaration.
- MMC1 / MMC3 CHR bank switching is in scope here but depends on the
  mapper work tracked under "Per-state background rendering control"
  and "Banked → banked cross-bank calls" above. Hybrid shims that
  simply pass through every CHR bank work today without bank-switch
  language support, since the original game code is still driving the
  switches.

### Fixed-point literals (already on the roadmap — pulled forward)

`fixed8.8` is listed in the Language feature gaps table. Finishing it
is a prerequisite for editable physics constants: without a
sub-pixel-capable numeric type, every decompiled physics constant is
an opaque `u8` and the user has to guess what "add 0x40 to vy each
frame" means. With `fixed8.8`, the same constant decompiles to
`gravity: fixed8.8 = 0.25` and is obvious at a glance. No work beyond
what that row already tracks, but it should be scheduled alongside the
decompiler rather than after.

### Tracker-shaped audio declarations

Already partially tracked under "Audio pipeline" (multi-channel tracker
playback, DMC, `@music("file.ftm")` importers). The decompiler-specific
ask is symmetric: once the language can *declare* a multi-channel track
with envelopes, vibrato, arpeggios, and DPCM samples, the decompiler
can *emit* those declarations by walking the original game's music
data tables through a driver-specific reader. FamiTone2 and its
derivatives are the natural first targets because they're used by a
large chunk of modern homebrew and have an open data format; Nintendo's
internal drivers are game-specific and will need per-title reverse
engineering, which the pattern library below is the place for.

Work beyond the audio-pipeline row:

- A stable binary schema for compiled tracker data that both the
  NEScript runtime driver and a FamiTone2-compatible driver can
  consume, so the same `music` declaration round-trips either way.
- An `asm_driver: famitone2` option on the `game` block that swaps
  in a FamiTone2-compatible tick routine instead of the NEScript
  runtime tick, for ROMs that want to use the format but keep
  NEScript's higher-level declarations.

### Goto escape hatch for lifted code

The hybrid shim is mostly data-driven, but we want the option to lift a
specific hot routine (player update, collision, enemy AI) to readable
`.ne` when it's worth the effort. 6502 control flow doesn't always fit
`if`/`while`/`for`, and insisting on structured lifting produces worse
output than admitting defeat on a few edges.

- `label foo:` statements inside function bodies.
- `goto foo` as an unconditional jump.
- Conditional `goto foo if <expr>` sugar, lowering to the normal
  branch-and-jump sequence.
- Analyzer rules: labels are scoped to their enclosing function; no
  cross-function gotos. Warning W0103 ("goto across a `var`
  declaration") for the cases where the structured equivalent is
  clearly better.
- The goto facility is opt-in and off by default: plain `.ne` programs
  should not reach for it. Gate it behind a `#[allow(unstructured)]`
  attribute on the containing function so code review catches
  accidental use.

### Free-space-aware replacement

When the user edits a pinned declaration and the recompiled blob no
longer fits in the original slot, the linker needs somewhere to put
the overflow. Commercial ROMs typically have ~10-30% unused padding
inside each bank; a decompiler can mark those regions and the linker
can relocate overflowing blobs into them, rewriting the pointer at
the original slot.

- `free_space @ 0x1EF00..0x1FFFA` declarations inside a `raw_bank`.
  The decompiler populates these from the ROM's observed $FF / $00
  runs longer than ~16 bytes.
- Linker pass: for each pinned declaration whose recompiled size
  exceeds its original slot, allocate from the nearest free-space
  region in the same bank, then rewrite the pointer at the original
  slot to point at the relocated blob.
- This only works when the decompiler knows *where* the pointer to
  the blob lives. For assets identified by pattern match against a
  known driver (FamiTone, the standard NES palette load loop, etc.)
  the pointer location is known. For everything else, pins are
  fixed-size only — overflow is a hard error.

### Symbol map import

Community disassemblies ship with `.sym` / `.mlb` files listing known
RAM and ROM labels. Feeding one into the decompiler turns every
`var_0x0090` into `player_x`, every `fun_C123` into `player_update`.
Huge readability win for zero extra analysis work.

- `nescript decompile --symbols mm2.mlb mm2.nes > mm2.ne` reads a
  Mesen-compatible `.mlb` (the same format `--symbols` already emits
  on the build side, so the parser in `src/linker/mod.rs::render_mlb`
  gains a symmetric reader).
- The decompiler names pinned declarations from the symbol table when
  available, falling back to `var_ADDR` / `fun_ADDR` otherwise.

### The `nescript decompile` subcommand

The actual decompiler, scaffolded as a new CLI subcommand alongside
`Build` / `Check` in `src/main.rs`. Produces a `.ne` file and the
binary blobs it references (`bank*.bin`, `chr*.png`) in a user-provided
output directory.

Staged delivery:

1. **Identity pass-through.** Read the iNES header, emit
   `game { ... }` + one `raw_bank` per PRG bank + one `chr_bank` per
   CHR bank + `raw_vectors` pointing at the original `$FFFA-$FFFF`
   table. Compile-runs to a byte-identical ROM. This is the proof
   point that the runtime-opt-out and raw-pass-through pieces are
   correct; it should land before any lifting is attempted.
2. **Pattern-matched asset lifting.** A per-driver pattern library
   that recognizes known palette-load loops, FamiTone header
   signatures, standard nametable init sequences, etc., and lifts
   the data blobs those routines read into `palette` / `music` /
   `sfx` / `background` declarations with `@`-pinned addresses. Each
   pattern is a self-contained module in `src/decompiler/patterns/`
   with a unit test that recognizes the pattern in a synthetic
   fixture and a golden test that recognizes it in a real ROM.
3. **Constant extraction.** Symbolic execution of specific hot
   routines (player update, one per supported game) to find
   `LDA #imm` loads into known physics RAM slots, then lift those
   immediates to `const` declarations pinned at their PRG offsets.
   Scope-limited to a short list of manually-identified routines
   per supported game — general-case constant extraction is out of
   scope.
4. **Optional structured lifting of flagged routines.** Goto-to-
   structured on routines the user explicitly opts into via
   `--lift 0xC123`. Uses the standard interval-analysis approach
   with the `goto` escape hatch for the cases that don't structure
   cleanly. Expect to iterate on quality here for a long time.

### Round-trip integration testing

The emulator harness under `tests/emulator/` is the natural oracle.
Add a new harness that, for each supported target ROM:

1. Runs `nescript decompile` on the ROM.
2. Runs `nescript build` on the decompiled `.ne`.
3. Runs both the original and the rebuilt ROM through the jsnes
   harness for the first ~10 seconds of autoplay.
4. Asserts the pixel-exact framebuffer and audio-hash goldens match.

Start with a corpus of one ROM (probably `examples/platformer.nes`
since it's already present and golden-tested) and expand as the
decompiler gains capability. Homebrew ROMs released under permissive
licenses are the natural next target; commercial ROMs can be tested
locally by contributors but should not be committed to the repo.

### Out of scope

- **Full structured lift of arbitrary 6502.** See the opening design
  note.
- **General-case constant and data-table recovery.** Every commercial
  ROM uses a different memory map; the pattern library is
  per-driver-per-game, not general.
- **Mapper-specific bank-switching code lifting.** Hybrid shims
  delegate to the original game's switching code; lifting it would
  require the full Tier-0 feature set for banked code the shim is
  specifically trying to avoid.
- **Copyright-bearing game ROMs as test fixtures.** Integration
  tests live against permissively-licensed homebrew only.

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
