# Future Work

This document catalogs known gaps, incomplete features, and planned improvements
in the NEScript compiler. Items are organized by priority and area.

---

## 1. IR-Based Code Generation

**Status**: Complete. The AST → IR lowering, optimizer, and
`src/codegen/ir_codegen.rs` all work end-to-end; the legacy AST
codegen has been removed. See "Recently completed" below.

---

## 3. Sprite Name Resolution

**Status**: `draw SpriteName at: (x, y)` parses the sprite name but codegen
ignores it. All draws use OAM slot 0 with CHR tile index 0 (the built-in smiley).

**What's needed**:
- Track a mapping from sprite name → CHR tile index in the linker
- When a `sprite` declaration provides inline CHR data or `@chr("file.png")`,
  write that data to the CHR ROM at a known tile index
- In codegen, look up the sprite name to get the tile index and write it to
  OAM byte 1 (tile number)
- Support multiple OAM slots: track a `next_oam_slot` counter per frame,
  allocate slots 0..63 as draws are emitted, warn if >64

---

## 4. Multi-OAM Sprite Support

**Status**: Every `draw` writes to the same OAM bytes ($0200-$0203). Only one
sprite is visible at a time.

**What's needed**:
- Frame-level OAM slot allocator: each `draw` gets the next available 4-byte slot
- Counter reset at the start of each frame handler
- Warn at compile time if a frame handler has >64 static draws
- Runtime: clear unused OAM slots to Y=$FE (off-screen) at frame start

---

## 5. State Machine Dispatch

**Status**: The codegen only generates code for the `start` state. Other states
are parsed and analyzed but their frame handlers are never called.

**What's needed**:
- A `current_state` zero-page variable holding the active state index
- A dispatch table at the start of the main loop: load `current_state`, branch to
  the corresponding state's frame handler
- `transition StateName` writes the new state index to `current_state`
- Generate `on_enter` call on transition, `on_exit` call before leaving

---

## 6. Include Directive

**Status**: The `include` keyword is lexed (`KwInclude`) but not parsed or
implemented.

**What's needed**:
- Parser: `include "path.ne"` at top level
- File resolution: relative to the including file's directory
- Circular include detection (track include stack)
- Merge included declarations into the main `Program` AST
- Span tracking: included files need their own `file_id` for error messages

---

## 7. Debug Mode

**Status**: `--debug` flag is accepted by the CLI but has no effect.

**What's needed**:
- `debug.log(expr, ...)`: write values to emulator debug port ($4800)
- `debug.assert(expr)`: emit runtime check, halt on failure
- `debug.overlay(x, y, text)`: render text to a reserved nametable region
- Array bounds checking in debug mode (compare index against array size)
- Frame overrun detection: count cycles per frame, warn if approaching vblank
- All debug code stripped in release mode (already designed: `DebugLog`/`DebugAssert`
  in the `Statement` enum are defined in the spec but not yet in the AST)

---

## 8. Scroll Hardware Writes

**Status**: `scroll(x, y)` is parsed but produces no output.

**What's needed**:
- Write X scroll value to PPU register $2005
- Write Y scroll value to PPU register $2005 (second write)
- Must happen during vblank (inside NMI handler or after `wait_frame`)
- Split-screen scroll requires MMC3 scanline IRQ (`on scanline`)

---

## 9. Asset Pipeline Completion

**Status**: PNG → CHR conversion exists (`src/assets/chr.rs`) but is never called
from the compilation pipeline.

### 9a. Wire `@chr("file.png")` to actual PNG loading
- When a sprite/background declares `chr: @chr("path.png")`, call `png_to_chr()`
  during compilation
- Resolve the path relative to the source file
- Store resulting CHR data in the ROM's CHR section at a known tile index

### 9b. Wire `@binary("file.bin")` to raw file inclusion
- Read the file as raw bytes and include in CHR or PRG ROM

### 9c. Palette extraction from PNG
- `@palette("file.png")`: analyze image colors, map to nearest NES palette entries
- Already have `nearest_nes_color()` in `src/assets/palette.rs`

### 9d. Nametable conversion
- Full 256×240 PNG → 960-byte nametable + 64-byte attribute table
- Tile deduplication (max 256 unique tiles per pattern table)
- Error if >256 unique tiles

---

## 10. Error Message Polish

**Status**: Errors work and render with ariadne, but many error paths use generic
messages.

### Unused error codes
These are defined in `ErrorCode` but never emitted:
- `E0202` — invalid cast
- `E0203` — invalid operation for type
- `E0301` — zero-page overflow
- `E0403` — unreachable state
- `E0505` — multiple start declarations
- `W0101` — expensive multiply/divide operation
- `W0102` — loop without break or wait_frame
- `W0103` — unused variable
- `W0104` — unreachable code after return/break/transition

### Missing validations
- No error for assigning to a `const`
- No error for `break`/`continue` outside a loop
- No warning for variables declared but never read
- No error for `return` with wrong type vs function signature
- No error for calling a function with wrong argument count/types

---

## 11. Scanline IRQ (MMC3)

**Status**: `on scanline(N)` is in the spec and `on_scanline` field exists in
`StateDecl`, but parsing and codegen are not implemented.

**What's needed**:
- Parser: `on scanline(N) { ... }` event handler in state bodies
- MMC3 IRQ setup: write scanline counter to $C000/$C001/$E000/$E001
- IRQ handler generation: branch to the scanline handler code
- Only valid with `mapper: MMC3`

---

## 12. Audio

**Status**: A full data-driven audio subsystem is in place. User programs
can declare `sfx Name { duty, pitch, volume }` blocks (frame-accurate
pulse-1 envelopes) and `music Name { duty, volume, repeat, notes }`
blocks (pulse-2 note streams with rests and looping). The resolver
compiles these into ROM-ready byte tables; the IR codegen emits
trigger sequences that load pointers into ZP; the runtime NMI tick
walks the envelope and note stream each frame, indexing into a
builtin 60-note period table. Builtin effects (`coin`, `jump`, `hit`,
`click`, `cancel`, `shoot`, `step`) and tracks (`theme`, `battle`,
`victory`, `gameover`) are synthesized from the same data path so
programs that don't declare their own audio still make sound.
Programs that touch no audio pay zero ROM or cycle cost — the whole
subsystem elides when the `__audio_used` marker is absent.

**Still TODO for richer audio**:
- Triangle/noise/DMC channels (currently only pulse 1 and 2 are used)
- Multi-channel tracker playback (one `notes` list per channel)
- `@sfx("file.nsf")` / `@music("file.ftm")` asset directives
- FamiTracker export format parsing
- Per-note pitch changes within a sfx (currently pitch is latched once)

---

## 13. Language Features (Post-v0.1)

From the spec's "Reserved for Future Versions" section:

| Feature | Description |
|---------|-------------|
| **Structs** | `struct Vec2 { x: u8, y: u8 }` — composite types with known layout |
| **Enums** | `enum Direction { Up, Down, Left, Right }` — mapped to u8 values |
| **Fixed-point** | `fixed8.8` type for sub-pixel movement |
| **Text/HUD** | Font sheet declarations, layout system for scores/health/menus |
| **Metasprites** | Multi-tile sprite groups with relative positioning |
| **Tilemaps** | Declarative level data with collision queries |
| **SRAM/saves** | Persistent storage declarations for battery-backed save data |
| **NES 2.0** | Extended iNES header format |

---

## 14. Inline Assembly

**Status**: `asm { }` blocks are lexed (`KwAsm`, `AsmBody`) but not parsed or
compiled. The lexer has raw-capture mode for asm content.

**What's needed**:
- Parser: capture asm body text, parse `{variable_name}` substitutions
- Codegen: emit raw 6502 instructions with variable address substitution
- Labels: local to the asm block scope
- `raw asm { }` variant with no substitution or safety checks

---

## 15. Compiler Performance

**Status**: Compilation is fast (<100ms for all examples) but has no benchmarks.

**What's needed**:
- `cargo bench` benchmarks for each pipeline phase
- Regression test: compilation must stay under 500ms for any reasonable project
- Profile-guided optimization of hot paths (lexer, parser)

---

## 16. WASM Build Target

**What's needed**:
- Factor out all file I/O behind a trait (`FileSystem` / `VFS`)
- Core pipeline takes `&str` source → `Vec<u8>` ROM with no filesystem access
- Compile the compiler to WASM for a browser-based IDE
- In-browser NES emulator integration for instant preview

---

## 17. Open Design Questions

From the engineering plan:

1. **Inline asm label syntax**: `.label:` (ca65 style) vs `label:` (generic)?
2. **Debug port address**: $4800 is conventional but not universal. Support
   multiple debug output methods?
3. **OAM allocation strategy**: Sequential allocation vs priority-based with
   automatic sprite cycling for the 8-per-scanline limit?
4. **Error recovery granularity**: How aggressively should the parser recover?
   More recovery = more errors per compile, but risk of cascading false errors.

---

## 18. Missing Assignment Operators

`<<=` and `>>=` (shift-assign) are lexed as `ShiftLeftAssign`/`ShiftRightAssign`
tokens but have no corresponding variants in the `AssignOp` AST enum. They can
never appear in parsed code. Adding them requires:
- New `AssignOp::ShiftLeftAssign` and `AssignOp::ShiftRightAssign` variants
- Parser handling in `parse_assign_or_call` (alongside the other compound ops)
- Codegen: load value, shift, store back

---

## 19. Player 2 Controller

`Player::P1` and `Player::P2` are defined in the AST but marked `#[allow(dead_code)]`.
The parser always produces `ButtonRead(None, ...)` — it never parses `p1.button.X`
or `p2.button.X` syntax. The runtime only reads controller 1 ($4016).

**What's needed**:
- Parser: `p1.button.X` and `p2.button.X` syntax producing `Player::P1`/`P2`
- Runtime: read controller 2 from $4017 into a second zero-page byte
- Codegen: select the correct input byte based on player

---

## 20. Register Allocator

The plan describes `src/codegen/regalloc.rs` for managing A/X/Y allocation, but
no register allocator exists. The current codegen uses A for everything and
spills to zero-page $02 for comparisons. A proper allocator would:
- Track A/X/Y liveness across basic blocks
- Use X for array indexing and loop counters
- Use Y as secondary index
- Spill to zero-page temps only when all three are live

---

## Priority Order

### Recently completed (removed from backlog)

These items were documented as future work but have since been implemented:

- **Full audio subsystem** — `src/runtime/mod.rs::gen_audio_tick`,
  `gen_period_table`, and `gen_data_block` implement a frame-walking
  pulse driver. `src/assets/audio.rs` compiles user `sfx`/`music`
  declarations (and builtins referenced via `play coin` etc.) into
  ROM-ready envelope and note-stream byte tables. `IrCodeGen::with_audio`
  threads the compile-time trigger constants into `play`/`start_music`,
  which emit pointer loads against per-blob labels. The linker splices
  driver body, period table, and every blob into PRG gated on the
  `__audio_used` marker so silent programs pay no cost. Full
  parser/analyzer/codegen/linker/runtime test coverage.
- **u16 arithmetic and comparisons** — new IR ops `LoadVarHi`,
  `StoreVarHi`, `Add16`, `Sub16`, `CmpEq16` through `CmpGtEq16`. The
  lowering context tracks variable types via the analyzer's symbol
  table and routes each expression through the 8-bit or 16-bit path
  based on operand width. Initializers, compound assignments, and
  comparisons all preserve both bytes. The codegen emits
  `CLC;ADC;ADC` for Add16 with carry propagating naturally, and
  compare-high-then-compare-low dispatch for the six comparison
  variants.
- **Multi-scanline on_scanline per state** — `gen_scanline_irq` now
  dispatches on `(current_state, ZP_SCANLINE_STEP)` and reloads the
  MMC3 counter with the delta to the next scanline in the same
  state. `gen_scanline_reload` resets the step counter at the top
  of each NMI so a state with multiple handlers fires them in
  ascending line order.
- **IR temp slot recycling** — `build_use_counts` pre-scans each
  function to count per-temp uses; `retire_op_sources` decrements
  the counts after each op runs and pushes dead slots back onto
  `free_slots` for later allocation. Previously, `bitwise_ops.ne`
  crashed (debug) or silently miscompiled (release) once it
  allocated more than 128 concurrent temps. With recycling the
  same function now uses ~4 slots instead of 136.
- **INC/DEC peephole fold** — `fold_inc_dec` collapses
  `LDA addr; CLC; ADC #1; STA addr` into a single `INC addr`
  (and the SEC/SBC/#1 variant into `DEC addr`). Saves 5 bytes and
  5 cycles per increment. The fold is suppressed when the next
  instruction is a carry-dependent branch (`BCC`/`BCS`) since
  INC/DEC don't update the carry flag.
- **Peephole dead-load elimination across passive ops** — the
  old `remove_dead_loads` only dropped an LDA if the very next
  instruction was another A-writer. Now it walks past
  INC/DEC/STX/STY (which don't touch A) to find the actual
  next A-use, catching more dead loads produced by copy
  propagation.

- **State machine dispatch** — CMP + BNE + JMP trampoline dispatch table,
  `current_state` in ZP $03, on_enter/on_exit handlers as JSR targets
- **Function call codegen** — JSR to function labels, ZP $04-$07 param passing,
  RTS for returns
- **Break/continue** — loop_stack with JMP to continue/break labels
- **Return** — evaluate expr to A + RTS
- **Transition** — write state index + JMP to main loop
- **Array indexing** — TAX + LDA/STA with ZeroPageX or AbsoluteX
- **Scroll** — PPU $2005 writes (X then Y)
- **Multiply/divide/modulo** — JSR __multiply/__divide with shift-and-add/restoring division
- **Shift left/right** — ASL A / LSR A
- **Multi-OAM sprites** — sequential slot allocation (0-63), reset per frame
- **Const assignment error** — E0203 for assigning to constants
- **Break outside loop error** — E0203 for break/continue without enclosing loop
- **Math routines wired into linker** — gen_multiply/gen_divide included in ROM
- **Sprite name resolution** — sprite declarations map to CHR tile indices,
  draw statements use the correct tile number
- **Inline sprite CHR data** — sprite decls with `chr: [0x..., ...]` work
- **Include directive** — `include "path"` inlines files at parse time,
  with circular include detection
- **Shift-assign operators** — `<<=` and `>>=` work in all contexts
- **Player 2 controller** — `p1.button.X` / `p2.button.X` syntax, P2 input
  read from $4017 into ZP $08
- **Unused variable warning** — W0103 emits for declared-but-never-read
  globals (underscore-prefix silences)
- **Unreachable state warning** — W0104 emits for states not reachable from
  the start state via transitions
- **E0502 "did you mean" suggestions** — undefined variable errors include
  a suggestion for nearby-named symbols
- **debug.log / debug.assert** — parses into `Statement::DebugLog` /
  `Statement::DebugAssert`, codegen emits runtime writes to $4800 when
  `--debug` is set, stripped in release mode
- **--debug CLI flag wired** — threads through `IrCodeGen::with_debug`
- **IR-based codegen** — `src/codegen/ir_codegen.rs` walks `IrProgram` and
  emits 6502 for every IR op: load/store, arithmetic, comparisons, arrays,
  calls, draws, input (P1 and P2), scroll, debug.log/assert, state
  dispatch, runtime OAM cursor for looped draws, transitions + on_enter
  handlers. It's the only codegen — the legacy AST-based path and the
  `--use-ast` flag were removed once the IR pipeline was proven correct
  by the jsnes emulator smoke test.
- **IR lowering bug fixes** — `ReadInput` now has a destination temp,
  `ButtonRead` uses the proper input temp, logical AND/OR use a new
  `emit_move` helper instead of the buggy raw VarId temp storage
- **IR Player 2 controller** — `ReadInput(temp, player)` selects $01
  or $08 based on player index
- **IR scroll support** — `scroll(x, y)` lowers to `IrOp::Scroll(x, y)`
  which emits two PPU $2005 writes in IR codegen
- **IR debug.log / debug.assert** — new `IrOp::DebugLog(temps)` and
  `IrOp::DebugAssert(cond)` variants, emitted as $4800 writes in
  debug mode and stripped in release
- **Asset pipeline @binary / @chr loading** — `resolve_sprites()` reads
  raw binary files and converts PNGs via `png_to_chr()`. Missing files
  are silently skipped (documentation-friendly)
- **Call arity validation** — E0203 when `Statement::Call` or
  `Expr::Call` has the wrong number of arguments or a mismatched
  argument type (uses a `function_signatures` map)
- **Return type validation** — `return value` is type-checked against
  the function's declared return type (E0201); returning a value from a
  void function emits E0203
- **W0102 loop-without-yield warning** — emitted when a `loop { ... }`
  body contains no `break`, `return`, `transition`, or `wait_frame`
- **W0101 expensive mul/div/mod warning** — flags multiply/divide/modulo
  with two non-constant operands; literal operands are silent because
  the optimizer strength-reduces them
- **W0104 dead-code-after-terminator warning** — statements after
  `return`, `break`, `continue`, or `transition` in the same block
  emit W0104 with a label pointing at the terminator
- **E0301 RAM overflow** — the zero-page user region is now bounded
  above by `$80` (leaving `$80-$FF` for IR temps) and the main RAM
  allocator stops at `$0800`; overflow emits E0301 at the declaration
- **E0505 multiple start declarations** — parser rejects a second
  `start X` token
- **`on scanline(N)` handlers** — `state { on scanline(240) { ... } }`
  parses and populates `StateDecl::on_scanline`; analyzer emits E0203
  if the game isn't using MMC3. The IR codegen now emits the full
  MMC3 IRQ vector glue: per-state dispatch in `__irq_user` and a
  `__ir_mmc3_reload` helper that picks the right `$C000` latch value
  based on `current_state`. See `examples/scanline_split.ne` and
  `examples/mmc3_per_state_split.ne`.
- **Inline assembly** — `asm { ... }` blocks. The lexer captures the
  body as a raw `AsmBody` token; `src/asm/inline_parser.rs` provides a
  minimal 6502 mnemonic parser that handles every addressing mode the
  codegen emits. The IR codegen splices parsed instructions directly
  into the output stream
- **Enum types** — `enum Name { V1, V2, ... }` declares u8 constants
  with values equal to declaration order. Variant names are flattened
  into the global symbol table
- **Struct types** — `struct Vec2 { x: u8, y: u8 }` with field
  access (`pos.x = 5`) and contiguous u8-only field layout. The
  analyzer synthesizes per-field VarAllocations so the rest of the
  compiler treats field access as ordinary variable access.
- **`for i in start..end { ... }` loops** — half-open range with a
  u8 index variable. Desugared in IR lowering to a while loop with
  a proper continue-edge block so `break`/`continue` work.
- **Audio subsystem** — full data-driven pulse driver with
  `sfx`/`music` block declarations, builtin effects, and an
  NMI-time tick that walks envelope and note-stream tables.
  See section 12 above for the full writeup.
- **Constant expression folding** — `const B: u8 = A + 3` evaluates
  at compile time and feeds through to variable initializers too.
- **`on scanline` codegen (minimal)** — MMC3 IRQ setup at startup
  plus a `__irq_user` dispatcher that saves registers, ACKs via
  `$E000`, dispatches on `current_state` to the right scanline
  handler, and restores. `__ir_mmc3_reload` helper re-arms the
  counter each frame from NMI.
- **Peephole optimizer** — `src/codegen/peephole.rs` runs to fixed
  point after codegen. Current passes:
  - copy propagation for IR temps (rewrites loads to their source)
  - dead-LDA elimination (drops overwritten LDAs)
  - redundant STA/LDA pair removal
  - LDA-then-STA-same-address removal
  - dead-store elimination for IR temp slots (function-wide scan)
  - A-value tracking eliminates redundant LDAs (ZP and absolute)
  - branch folding: `Bxx L; JMP M; L:` → `Byy M`
  - dead JMP to next label removal
- **`--dump-ir` CLI flag** — prints the lowered IR program after the
  optimizer pass for debugging
- **Function-local variables** — IR codegen allocates backing
  storage for `var`s declared inside function bodies, using a
  per-function RAM range at `$0300+` so nested calls don't clobber
  each other.
- **E0502 on assignment to undefined variable** — previously was
  silently creating a new variable.
- **Function call ABI fix** — IR codegen was JSRing to `__fn_name`
  but functions were defined as `__ir_fn_name`, and param VarIds
  weren't in `var_addrs` so callees read temp slots instead of
  parameters. Both bugs are now fixed with an integration test
  guard.
- **Struct literal syntax** — `Vec2 { x: 100, y: 50 }` in both
  variable initializers and assignments. Desugars in lowering to
  per-field stores. Restricted to non-condition expression contexts
  (if/while/for conditions) to avoid ambiguity with block `{`.
- **Match statement** — `match x { pat => body, _ => default }`
  parses to an if/else-if chain at parse time, so no new AST
  variant is needed. Supports any expression patterns and an
  underscore catch-all.
- **For loops** — `for i in start..end { body }` (half-open range).
  Desugars in IR lowering to a while loop with a proper
  continue-edge block.
- **Semicolon statement separators** — short statements can share
  a line: `a += 1; b += 2`.
- **Inline asm `{var}` substitution** — inside an `asm { ... }`
  block, `{name}` is replaced with the hex address of the variable
  `name`. The lexer balances nested braces so `{counter}` inside
  an asm body is captured correctly.
- **`raw asm { ... }` blocks** — variant of inline asm that skips
  `{var}` substitution, passing the body through verbatim.
- **`poke(addr, value)` / `peek(addr)` intrinsics** — hardware
  register access without needing an asm block. Compile to a
  single LDA/STA against a compile-time-constant address.
- **`--memory-map` CLI flag** — prints a human-readable variable
  allocation table showing what's in ZP vs main RAM.
- **`--call-graph` CLI flag** — prints a call-tree view with max
  depth reached from each entry point handler.

### Remaining priority order

For someone picking up this codebase, the recommended order of work:

1. **u16 / array / nested struct fields** — u16 *globals* now work
   end-to-end (load/store, +/-, comparisons all propagate through
   16-bit IR ops). Struct fields and array elements are still u8-only
   — the layout machinery needs to grow multi-byte field offsets.
2. **Triangle / noise / DMC channels** — the current audio engine
   plays sfx on pulse 1 and music on pulse 2 with a full
   data-driven tracker model (envelope walk, period table,
   `(pitch, duration)` note streams, loop-back). Wiring triangle
   and noise channels into the same model would unblock richer
   multi-part compositions.
3. **Register allocator** — proper A/X/Y allocation to replace
   zero-page spills used by the current IR codegen. Partially
   mitigated by peephole passes + the new slot recycler, but still
   wasteful in some cases (every temp spills to a ZP slot even if
   its live range is one op wide).
4. **Text / HUD layer** — font sheet + layout system for scores.
5. **Cross-block temp live-range analysis** — the current slot
   recycler is function-local; temps that flow across block
   boundaries always get a dedicated slot for the full function.
   A proper CFG-aware live range interference graph would let more
   temps share slots.
6. **Peephole: drop LDA dead across unconditional JMPs** — after
   the INC/DEC fold we sometimes leave an `LDA #1` whose value is
   consumed by nothing before the next `JMP __ir_main_loop`. Local
   analysis can't prove it's dead; a cross-block pass could.
