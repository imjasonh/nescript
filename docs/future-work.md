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
entries for every lowered statement. **`--dbg <path>` writes a
ca65-compatible `.dbg` debug-info file** that Mesen / Mesen2 / fceuX pick
up automatically for source-level stepping, labelled variable inspection,
and symbol-based breakpoints. The file stitches together the linker's
label table, the `__src_<N>` IR markers, and the analyzer's variable
allocations into the `file`/`mod`/`seg`/`scope`/`span`/`line`/`sym`
records documented at
<https://cc65.github.io/doc/debugfile.html>. `ooffs` on the segment
record tracks the fixed bank's PRG-relative start, so banked ROMs
(MMC1/UxROM/MMC3) also map cleanly inside the debugger. Debug builds emit array bounds checks
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

### State-local memory overlay follow-ups

State-local variables are now overlaid across mutually-exclusive states
(see the analyzer's per-state allocation cursor rewind and the IR
lowerer's `on_enter` initializer prologue), but a few pieces are still
missing:

- **Same-named locals across different states.** `register_var` stores
  state-locals under their bare name, so two states each declaring
  `var timer: u8` collide with E0501. A per-state symbol-table scope
  prefix would let each state carve its own namespace while keeping
  the overlay.
- **Struct-literal and array-literal initializers on state-locals.**
  The on-enter prologue lowers scalar initializers cleanly, and
  struct-literal initializers fall back to per-field stores, but
  array-literal initializers (`var xs: u8[4] = [1,2,3,4]`) are
  skipped. A runtime `memcpy` from a ROM blob into the overlay
  slot (mirroring the reset-time global path) is the natural
  lowering.
- **Handler-local overlay.** Handler-local `var`s declared inside
  `on_frame { ... }` are already per-handler scoped via
  `current_scope_prefix`, but they get a dedicated RAM slot for the
  program's lifetime. Overlaying them inside each handler's stack
  frame — using a per-handler bump allocator that resets on each
  call — would shave a few bytes more on programs with many deep
  handlers.

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

## cc65/nesdoug parity gaps

The nesdoug tutorial series + neslib expose a broad surface that
NEScript can't currently express. This section enumerates the gaps
in the order they should probably be tackled (cheapest/highest-
leverage first). Anything we finish moves out of this section and
into the top of the file with a "ships today" note.

### A. Numeric types beyond `u8 / i8 / u16 / bool`

- **`i16`.** The smallest change with the highest blast radius.
  Negative metasprite offsets, signed velocities, signed scroll
  deltas, subtraction-of-positions — none of that works today
  without underflow hazards. Design sketch:
  - Lexer: no change (type names are already identifiers).
  - Parser: add `i16` to the primitive-type list.
  - Analyzer: extend `Type` + coercion table; signed×unsigned
    mixing should require an explicit cast.
  - IR: `Add/Sub/Cmp` already carry a signedness flag for `i8`;
    extend the 16-bit variants the same way.
  - Codegen: `CMP`/`BCC`/`BCS` for unsigned vs `BMI`/`BPL`/signed
    compare for signed (XOR the high bits before subtract, or
    branch on the overflow flag).
- **`u32` / `i32`.** Lower priority; realistically needed only for
  score totals and frame counters. A synthesizable pair of
  16-bit halves is usually enough.

### B. Pointers & function pointers

NEScript has no pointer type. This blocks indirect-dispatch
tables (`jmp (vec,x)`), variable-size buffer manipulation, and
passing "which thing" to a helper. cc65's `__fastcall__` function
pointers via wrapped-call + bank IDs are load-bearing for every
game over 32 KB. Design sketch:

- Introduce `*T` / `fn(T) -> U` type grammar.
- Spell a new IR op `CallIndirect` that takes an address in a
  16-bit temp, plus a `BankHint` so cross-bank pointers trampoline
  automatically.
- For fixed-bank-only code we can lower to a raw `JSR ($vec)`
  equivalent (`JMP ($vec)` + return stub).

### C. Bitfields and unions

OAM attribute bytes, controller masks, collision-flag words, and
MMC3 register bits all want bitfield syntax (`struct OamAttr { pal:
u2, priority: u1, flip_h: u1, flip_v: u1 }`). Unions show up less
often but are useful for reading/writing the same bytes as two
different shapes (e.g. a 16-bit counter viewed as `lo: u8, hi: u8`).

### D. Full inline-assembly escape hatch

- Today the inline-asm lexer accepts `label:` but not `.label:`
  (ca65 style). Port the lexer to accept `.`-prefixed local
  labels and emit them as ca65-compatible locals (`@label`).
- Accept cc65's `asm()` format specifiers — `%b` (byte), `%w`
  (word), `%l` (long), `%v` (var), `%o` (offset), `%g` (global),
  `%s` (string) — so users can splat a compiler-allocated symbol
  into a hot-loop fragment.
- Extend the directive allow-list: `.byte`, `.word`, `.res`,
  `.repeat / .endrep`, `.macro / .endmacro`. The assembler can
  already encode these.

### E. Dense-`match` jump tables

`match u8 { 0 => ..., 1 => ..., ... }` desugars to an if/else
chain at parse time today, which is `O(n)` compares. For dense
(<= 256 entries with <= 4× spread) integer matches, lower to:

```
ASL A           ; index *= 2
TAX
LDA table_lo,X
STA $00
LDA table_hi,X
STA $01
JMP ($0000)
```

…with a per-branch `.word` table emitted in the function prologue.
This matters most for state dispatch and attack/weapon tables.

### F. Recursion stance (design constraint, not a bug)

The analyzer rejects recursive calls with E0402. That's the right
call for a compiler targeting a 6502 hardware stack, but it's not
documented as a **design choice** anywhere. Add a paragraph to the
language guide explaining why, plus a pointer to the hand-rolled
explicit-stack pattern (small `u8[N]` stack + `u8` top).

### G. VRAM update buffer primitive

The highest-leverage missing runtime feature. Today
`load_background` / `set_palette` queue PPU writes under the hood,
but there is no user-visible "write these N bytes into nametable
slot `(x,y)` next vblank" primitive. That's the idiom behind every
scoreboard, dialog box, destroyed-metatile animation, and streaming
scroll in the nesdoug chapters. Concrete API sketch:

```
buffer.nametable_write(x, y, [0x20, 0x21, 0x22])   // horizontal
buffer.nametable_write_v(x, y, [0x20, 0x21, 0x22]) // vertical
buffer.attribute_write(x, y, 0b00011011)           // one byte
buffer.flush()                                     // force an eof
```

Runtime shape: a fixed ring buffer at a known RAM address
(`$0400`?). Each entry is `[header, addr_hi, addr_lo, len, data…]`
where `header` carries the `NT_UPD_HORZ` / `NT_UPD_VERT` /
`NT_UPD_EOF` bits the neslib engine already uses. The NMI handler
drains the buffer every frame and writes `$FF` as the sentinel.

### H. Metatiles + collision as a first-class construct

cc65/nesdoug treats 2×2 metatiles + a parallel collision map as
the core room format. `docs/future-work.md` mentions "tilemap
collision queries"; raise the scope to a single cohesive feature:

```
metatileset DirtWorld {
  source: @tiles("dirt.chr"),
  metatiles: [
    { id: 0, tiles: [0, 1, 16, 17], collide: false },
    { id: 1, tiles: [2, 3, 18, 19], collide: true  },
    ...
  ],
}

room Level1 {
  metatileset: DirtWorld,
  layout: @room("level1.nxt"),  // NEXXT exporter format
}

on_frame {
  if collides_at(hero.x, hero.y) {
    ...
  }
}
```

The compiler would expand each `room` into a packed `[(metatile_id
<< 4 | collision_bits), ...]` blob in PRG ROM, emit a
`collides_at(x: u16, y: u16) -> bool` helper, and stream the
expanded tiles into the VRAM update buffer on a `paint_room()` call.

### I. RLE + LZ4 nametable decompression

`vram_unrle` and `vram_unlz4` — for scrolling/multi-room games,
packing rooms is mandatory. cc65 ships both in neslib with
concrete timing (0.5f RLE vs 2.8f LZ4). The per-state background
swapping item in "What ships today" is exactly this problem:
without a decompressor that can stream into the VRAM buffer, the
NMI-time write budget (~2273 cycles) is too tight for a full
nametable. RLE is the smaller first step — emit a `nametable` that
can declare `compression: rle` and decompress at swap time.

### J. Palette brightness / fade

Neslib's `pal_bright(level: 0..8)` is a one-call fade that flips
the PPU mask emphasis bits and optionally darkens the active
palette via a brightness LUT. One-call fade-in / fade-out is
enormous polish for nearly no runtime cost. API:

```
builtin set_palette_brightness(level: u8)   // 0=off, 4=normal, 8=white
builtin fade_out(frames: u8)                // blocks; drives mask bits
builtin fade_in(frames: u8)
```

### K. Edge-triggered input

Today NEScript exposes level-state buttons (`p1.a` is whatever the
hardware reports this frame). Every menu needs a "just pressed
this frame" primitive. Extend the button type with `.pressed` and
`.released` accessors:

```
if p1.a.pressed { menu_accept() }
if p1.start.released { pause_menu() }
```

Implementation is one more ZP byte per controller (`p1_prev`) and
an XOR in the input polling stub.

### L. Sprite 0 hit split-screen

`split(x, y)` is the neslib primitive for a fixed status bar above
a scrolling playfield without MMC3. NEScript only offers
`on_scanline(N)` on MMC3. A sprite-0-hit-based split that works on
NROM/UxROM/MMC1 unlocks most of the tutorial games. API:

```
sprite_0_split scanline: 32, {
  scroll_x: 0,
  scroll_y: 0,
}
```

…emits a busy-wait on `$2002` bit 6 followed by the requested
scroll write.

### M. Automatic sprite cycling

The existing `cycle_sprites` opt-in keyword rotates the DMA offset
each frame. A `game { sprite_flicker: true }` attribute that emits
the rotation automatically — plus a `draw ... priority: pinned`
modifier for HUD sprites that must stay at low OAM slots — is the
cleaner user-facing API. Mentioned already under Open Design
Questions; bumping it into the active roadmap.

### N. Runtime PRNG

`rand8()` / `rand16()` / `set_rand(seed: u16)` are in every
nesdoug demo. Implement as a ZP-held 16-bit LFSR (xorshift16 is
tiny and good enough). Users shouldn't have to hand-roll this.

### O. DPCM / DMC sample playback

Already listed under Audio Pipeline. FamiStudio's DMC support
(including bankswitched DMC) is the reference API shape — import
`@dpcm("file.dmc")` into a named sample slot and expose
`play_dpcm(Slot, pitch: u8, loop: bool)`.

### P. Expansion audio (VRC6, MMC5, FDS, N163, S5B, VRC7)

FamiStudio has a single export path with `FAMISTUDIO_CFG_EXTERNAL`
and per-chip feature flags. If/when we import a FamiStudio-export
format (see Q), the expansion chips come along almost for free
— the runtime just has to wire up the extra write ports and the
mapper has to expose them (MMC5 for the extra pulse channels,
VRC6/VRC7 via their own mappers).

### Q. FamiStudio text-export import

`@music("file.famistudio.txt")`. FamiStudio's text export is the
pragmatic ingestion path; parsing it gives full tracker semantics
(volume/pitch slides, arpeggios, vibrato, release notes) without
reinventing the engine. FamiTracker's binary `.ftm` is a worse
target — undocumented, version-skewed.

### R. NEXXT metatile/collision import

NEXXT is the dominant asset editor in the nesdoug workflow; it
emits metatile tables + collision maps as ca65-compatible
assembler source. An `@metatiles("room.nxt")` loader (and
`@room("level1.nxt")` for layouts — see §H) removes a whole class
of hand-typed tile arrays.

### S. SRAM / battery-backed saves

Already in the spec as a "reserved for future versions" item. Add
a top-level `save { var … }` block that lands its allocations at
`$6000+`, flips the iNES battery flag, and exposes the allocations
to the rest of the program as if they were ordinary globals (with
a compiler-emitted checksum on write to survive cold starts).

### T. PAL/NTSC region abstraction

Neslib exposes `ppu_wait_frame` as a virtual-50Hz wait on PAL. Add
a `region: ntsc | pal | dual` field on the `game { }` block. For
`dual`, the runtime probes `$2002` bit 7 timing at reset and sets a
ZP flag; the audio engine's frame tick and any frame-counted timing
respects the flag.

### U. Additional controller types

Expose Zapper (light-gun) and Power Pad via typed inputs:

```
input gun: zapper on port: 1
input mat: power_pad on port: 2
```

`gun.trigger`, `gun.light_detected`, `mat.button(i: u8)` are the
three reads every program needs.

### V. Additional mappers

In priority order (cheapest × highest demand first):

1. **AxROM** (mapper 7). Single-screen mirroring, up to 256 KB PRG
   bankswitched in 32 KB pages. Almost a trivial extension of the
   UxROM path — one register, different mirroring bit.
2. **CNROM** (mapper 3). 8 KB CHR bankswitching, fixed 32 KB PRG.
   One register, CHR-only. Also trivial.
3. **GNROM / MHROM** (mapper 66). Combines AxROM-style PRG with
   CNROM-style CHR banking. Another single-register mapper.
4. **MMC2** (mapper 9, Punch-Out only realistically). Medium.
5. **UNROM-512** (mapper 30). The modern homebrew sweet spot —
   512 KB PRG + CHR-RAM + self-flashing. Mapping is UxROM-like
   plus a one-screen bit.
6. **MMC5** (mapper 5). Big. Driven by FamiStudio's expansion
   audio more than by the extra PRG/CHR modes. Probably last.

### W. NSF output target

The audio engine is already a standalone subsystem. An NSF-output
target (`--target nsf`) would wrap the existing music/sfx blocks
in the NSF header and expose `init`/`play` entry points. Nearly
free, gets the chiptune audience for ~a day of work.

### X. Configurable / Mesen-native debug output

Today the debug port is hardcoded to `$4800`. Expose
`debug.port: $4800 | mesen` on the `game { }` block. For
`mesen`, emit writes to `$4018` (Mesen's documented debug port)
and document the trace-log tool invocation in the debug docs.

### Y. FCEUX `.nl` / `.ld` label file output

`--dbg` writes ca65-compatible debug info, which Mesen + Mesen2 +
FCEUX all consume for source-level stepping. FCEUX also supports
its native `.nl` (per-bank label) / `.ld` (line) files, which some
users prefer. Cheap addition: `--fceux-labels <prefix>` emits
`<prefix>.0.nl`, `<prefix>.1.nl`, …, `<prefix>.ld`.

### Z. Explicit bank-placement hints on functions and data

`bank Foo { fun bar() }` already exists; extend the sugar to
attributes on individual items so users don't have to restructure
their source:

```
@bank(3) fun slow_helper() { ... }
@bank(3) const LEVEL_DATA: u8[1024] = [...]
```

This is particularly useful for `const` data, which today lands
wherever the analyzer decides; users sometimes need to pin data
to a specific bank to avoid bank-switch cost on a hot path.

### Priority ranking

In practice the order I'd tackle these for maximum user value:

1. `i16` (§A) — unblocks signed physics, metasprite offsets.
2. VRAM update buffer (§G) — unblocks HUDs, dialog, streaming.
3. Edge-triggered input (§K) + PRNG (§N) — one-line demo wins.
4. Palette fade (§J) + sprite-0 split (§L) — cheap polish wins.
5. Register allocator (existing section) — compounding size win.
6. Metatiles + collision (§H) — closes several items at once.
7. Inline-asm completeness (§D) — escape hatch for power users.
8. Arrays-of-structs + bitfields (§C) + fn pointers (§B) —
   turns NEScript into a general-purpose NES language.
9. SRAM (§S) + AxROM/CNROM/UNROM-512 (§V) — ecosystem fit.
10. FamiStudio import (§Q) + DPCM (§O) + expansion audio (§P).

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
