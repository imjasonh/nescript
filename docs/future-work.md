# Future Work

This document tracks the gaps between what NEScript currently compiles and
what the spec describes. Items are grouped by area. Anything implemented and
tested is omitted — `git log` is the authoritative record of what shipped.

---

## Runtime palette / nametable updates

**Status.** Parsed away. Previously the compiler accepted `palette Name
{ colors: [...] }`, `background Name { chr: @chr(...) }`, `load_background
Name`, and `set_palette Name` statements, but the lowering silently dropped
them: the declarations were never resolved into CHR or palette blobs and the
statements emitted zero instructions. They were removed during cleanup to
avoid quietly misleading users.

**What a proper implementation needs.**
- New AST declarations for palette and background/nametable data, with
  analyzer validation for color indices and nametable dimensions.
- An asset pipeline pass that compiles PNG or inline byte data into a
  ROM-resident blob with a known label.
- Runtime helpers that write to PPU `$2006/$2007` during vblank from a
  source pointer.
- IR ops `LoadBackground(BlobId)` and `SetPalette(BlobId)` that the
  codegen emits inside an NMI-safe window.
- A `--memory-map` breakdown that shows palette and nametable budgets,
  since they eat into the PRG-ROM cost model that `memory_map` reports
  for CHR bytes.

Until this exists, programs should use `poke(0x2006, ...)` / `poke(0x2007, ...)`
directly inside an `on frame` handler (runs immediately after vblank) to push
palette or nametable updates.

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
