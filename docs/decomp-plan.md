# Decompilation Implementation Plan

This document describes the work required to ship a hybrid-shim decompiler that can parse existing .nes ROMs, extract structured data (assets, constants, audio), and emit editable .ne source that round-trips through the emulator golden tests.

**Scope:** NEScript-produced ROMs only (fingerprinting the runtime), hybrid approach (raw code pass-through + lifted assets), phased rollout with measurable milestones.

---

## Milestone Overview

| # | Milestone | Effort | Dependencies | Success Criteria |
|---|-----------|--------|--------------|------------------|
| M1 | Language foundations: address-pinned decls, raw_bank, raw_vectors, fixed8.8 | ~4 days | none | All parser/analyzer/linker tests pass; examples compile unchanged |
| M2 | goto/label escape hatch for decompiler code | ~2 days | M1 | Parser/analyzer accept and strip `#[allow(unstructured)]`; clippy clean |
| M3 | nescript decompile subcommand (identity + simple lifts) | ~5 days | M1, M2 | Cmd accepts `.nes` file; outputs identity-pass-through `.ne` |
| M4 | FamiTone2 driver recognition + audio data extraction | ~3 days | M3 | Correctly parses pulse/triangle/noise/dmc period tables; extracts period indices |
| M5 | Round-trip integration tests | ~2 days | M3, M4 | Decompile → recompile → emulator diff all examples/*.nes; goldens match |

**Total:** ~16 days of focused work. Feasible as 2-3 parallel agent-weeks (agents work nights/weekends if needed).

---

## Milestone 1: Language Foundations

**Objective:** Extend the parser, analyzer, and linker to support decompiler-specific declarations without breaking existing programs.

### 1.1 Address-pinned declarations: `@ 0xADDR` syntax

**Parser (src/parser/ast.rs, mod.rs):**
- Extend `Placement` enum:
  ```rust
  pub enum Placement {
      Fast,
      Slow,
      Auto,
      Fixed(u32),      // PRG offset or ZP address for const/var
      PrgOffset(u32),  // Explicit PRG offset
  }
  ```
- Update `VarDecl` and `ConstDecl` to optionally hold an address:
  ```rust
  pub struct VarDecl {
      pub name: String,
      pub var_type: NesType,
      pub init: Option<Expr>,
      pub placement: Placement,
      pub address: Option<u32>,  // If pinned, exact address
      pub span: Span,
  }
  ```
- Update parser grammar to accept `var x: u8 @ 0x1234 = init;`
- Tests: parse address-pinned var/const/palette/background/sfx/music declarations and round-trip through a simple identity recompile.

**Analyzer (src/analyzer/mod.rs):**
- When `address` is present, skip the normal zero-page/PRG allocator for that item. Use the pinned address instead.
- Collision detection: warn if two pinned addresses overlap (W0109 "decompiler-only construct in hand-authored source").
- For ZP-pinned items, skip the runtime ZP range check ($00-$0F reserved, $11-$17 if palette/bg, user at $10+/$18+).
- Tests: pinned vars don't collide with runtime ZP; pinned PRG items place correctly in final ROM.

**Linker (src/linker/mod.rs):**
- When laying out PRG, honor pinned offsets: if an item has `address`, write it at that exact offset, not in the normal sequential layout.
- Panic if a pinned offset would overlap with runtime code or another pinned item.
- For zero-page: place pinned ZP items at their specified address, skip those addresses in the normal allocator.
- Tests: pinned ROM items land at the right byte offsets; roundtrip ROM matches input layout.

### 1.2 raw_bank pass-through

**Parser & AST:**
- Add `RawBankDecl` to Program:
  ```rust
  pub struct RawBankDecl {
      pub index: u8,
      pub source: AssetSource,  // Binary file
      pub span: Span,
  }
  pub banks: Vec<RawBankDecl>,  // New field in Program
  ```
- Syntax: `raw_bank Name @ 0 { binary: "path.bin" }`
- Tests: parse and emit; no semantic analysis.

**Linker:**
- When laying out PRG banks, if a bank is marked raw, read the entire binary file and splice it at the correct bank offset without instruction parsing.
- Tests: raw_bank content appears byte-for-byte in output ROM.

### 1.3 raw_vectors + runtime opt-out

**Parser & AST:**
- Add optional `raw_vectors` block to GameDecl:
  ```rust
  pub raw_vectors: Option<RawVectors>,
  pub struct RawVectors {
      pub reset: Option<u16>,  // Absolute address in PRG
      pub nmi: Option<u16>,
      pub irq: Option<u16>,
  }
  ```
- Syntax: `game Foo { mapper: NROM, raw_vectors { reset: 0xC123, nmi: 0xC000 } }`
- Tests: parse; do not emit default vectors when raw_vectors is present.

**Codegen & Linker:**
- In `runtime::gen_init`, check if `raw_vectors` is present. If so, skip emitting the vector table at $FFFA-$FFFF.
- Linker: if raw_vectors specified, emit the given addresses (exactly as-is) to the vector table instead of auto-calculated labels.
- Tests: vector table holds user-specified addresses; no gen_init boilerplate when opted-out.

### 1.4 free_space regions

**Parser & AST:**
- Extend `RawBankDecl` to include optional free space list:
  ```rust
  pub struct RawBankDecl {
      pub index: u8,
      pub source: AssetSource,
      pub free_space: Vec<(u32, u32)>,  // Vec of (start, end) offset ranges
      pub span: Span,
  }
  ```
- Syntax: `raw_bank Bank0 @ 0 { binary: "file.bin", free_space: [(0x2000, 0x3000), ...] }`
- Tests: parse; optional field, defaults to empty vec.

**Linker:**
- When a pinned PRG item doesn't fit in its original slot (happens if decompiled source was edited to grow an asset), try to fit it in a free_space region instead.
- If no free_space region can fit it, emit a diagnostic saying which item overflowed and which ranges are available.
- Tests: item that fits in original slot stays; item too large is relocated to free_space; relocation fails with clear error if no space.

### 1.5 fixed8.8 type

**Parser (src/parser/ast.rs, mod.rs):**
- Add variant to `NesType`:
  ```rust
  pub enum NesType {
      U8, I8, U16, Bool,
      Array(Box<NesType>, u16),
      Struct(String),
      Fixed8p8,  // New: signed fixed-point Q8.8
  }
  ```
- Parser: accept `fixed8.8` as a type keyword.
- Literal syntax: `0x12.34` parses as a fixed-point literal (0x12 integer part, 0x34 fractional). Alternatively `1.25` as a decimal; internally stored as u16 (0x0140 for 1.25).
- Tests: parse fixed literals; reject invalid formats.

**Analyzer (src/analyzer/mod.rs):**
- fixed8.8 is u16 under the hood; allocate 2 bytes.
- Operators: `+`, `-`, `*` (with 8-bit truncation), `/` (integer division), comparison.
- Tests: fixed8.8 variables allocate 2 bytes; arithmetic ops resolve correctly.

**Codegen (src/codegen/ir_codegen.rs):**
- `+`: add high + low bytes with carry handling.
- `-`: subtract with borrow.
- `*`: u8*u8 → u16, store result.
- `/` and `%`: divide high by high (approximate, standard fixed-point division).
- Comparison: compare high byte first, then low if equal.
- Tests: codegen produces correct 6502 sequences; emulator tests with fixed-point math pass.

### 1.6 Test coverage for M1

- **Parser tests:** All new syntax (pinned addresses, raw_bank, raw_vectors, fixed8.8) parses and rejects invalid forms.
- **Analyzer tests:** Pinned items don't collide; layout is correct; fixed8.8 typing works.
- **Linker tests:** Pinned ROM items write at correct offsets; raw_bank splices verbatim; vector table respects raw_vectors.
- **Integration test:** Recompile an existing example with pinned constants, verify ROM matches expected byte offsets.

---

## Milestone 2: goto/label Escape Hatch

**Objective:** Allow decompiler to emit unstructured code (goto/label) gated behind `#[allow(unstructured)]`.

### 2.1 Parser & Analyzer

**Parser (src/parser/ast.rs, mod.rs):**
- Add to statement enum:
  ```rust
  pub enum Statement {
      // ... existing variants
      Label { name: String, span: Span },
      Goto { target: String, span: Span },
  }
  ```
- Syntax: `label loop_start:` and `goto loop_start;`
- Parser should accept these only if the function/state handler has `#[allow(unstructured)]` attr.
- Tests: parse goto/label with attribute; reject without attribute.

**Analyzer (src/analyzer/mod.rs):**
- Build a label table per function.
- Resolve all goto targets to ensure they exist and are in the same function.
- W0109 diagnostic if unstructured code appears without the attribute.
- Tests: goto/label resolution works; missing targets or attributes caught.

### 2.2 Codegen (src/codegen/ir_codegen.rs)

- `Label` statement → emit a label in the asm output (no runtime cost).
- `Goto` statement → `JMP <label>`.
- No special flow-analysis needed; goto is a valid terminal for blocks (already handled via JMP).
- Tests: codegen emits correct label/JMP sequences; emulator roundtrips.

### 2.3 Peephole & Optimizer

- Dead-code elimination should not remove labels that are targets of gotos.
- If a goto is unconditional and only reachable via goto (not by fall-through), the intermediate block can be folded into the goto target (optional; skip for MVP).
- Tests: gotos are not eliminated; reachability analysis respects labels.

### 2.4 Test coverage for M2

- **Parser tests:** goto/label with and without `#[allow(unstructured)]`.
- **Analyzer tests:** Label table, goto resolution, collision detection.
- **Codegen tests:** Label/JMP emission.
- **Emulator test:** An example with goto-based loop structure roundtrips through goldens.

---

## Milestone 3: nescript decompile Subcommand

**Objective:** Add `nescript decompile <rom> [-o output.ne]` that produces identity-pass-through .ne source.

### 3.1 Main entry point (src/main.rs)

- Add `decompile` subcommand to the CLI.
- Parse `.nes` file header to extract mapper, mirroring, CHR size, etc.
- Emit skeleton .ne:
  ```rust
  game "DecompiledROM" {
      mapper: NROM,  // or the detected mapper
      mirroring: horizontal,  // detected from header
  }
  raw_bank Bank0 @ 0 { binary: "original.prg.bin" }
  // (CHR follows similarly if present)
  ```
- Tests: decompile hello_sprite.nes → .ne; compile it back; compare ROM.

### 3.2 Module structure (new src/decompiler/)

Create the decompiler as a pluggable subsystem:

```
src/decompiler/
  mod.rs           // Main pipeline: fingerprint → lift assets → emit .ne
  patterns/
    mod.rs         // Pattern matching registry
    famitone.rs    // FamiTone2 driver recognition
    nes_runtime.rs // NEScript runtime fingerprinting
    audio.rs       // Audio data table extraction
  lifter.rs        // Asset lifting (CHR → PNG, palette, nametable, etc.)
  emitter.rs       // .ne source generation
  tests.rs
```

### 3.3 NEScript runtime fingerprint (src/decompiler/patterns/nes_runtime.rs)

- At program load, scan PRG for byte sequences matching NEScript's init/NMI/IRQ/audio handlers (from `src/runtime/mod.rs`).
- If a match is found, mark the ROM as "NEScript-produced" and extract:
  - User code start offset (just after gen_init)
  - State dispatch table location (__ir_main_loop)
  - OAM upload range
  - Palette/background update ranges
- If no match, emit a warning and proceed with raw_bank pass-through only (conservative).
- Tests: correctly identify NEScript ROMs; fingerprint a couple of real examples.

### 3.4 Identity pass-through

- Read PRG bank(s) as binary, emit `raw_bank` declarations.
- Read CHR bank(s) as binary (or convert to PNG for convenience).
- Emit game declaration with detected mapper/mirroring.
- Compile result and byte-compare ROM: should be identical.
- Tests: identity round-trip for all examples/*.nes.

### 3.5 Simple asset lifting (M3 MVP)

Start with minimal pattern matching:

- **CHR extraction:** Slice CHR banks from the ROM, optionally convert to PNG (via `pngjs` or a simple row-major encoder).
- **Palette extraction:** Find the 32-byte palette blob (usually near the start of fixed PRG). Emit as `palette Default { colors: [...] }`.
- **Nametable extraction:** Find 960+64 byte nametable blobs and emit as `background Name { tiles: [...], attributes: [...] }`.

No decompilation of code yet; just binary search/pattern match to pull out data blobs.

Tests: extracted palettes/nametables match originals when recompiled.

### 3.6 Test coverage for M3

- **Unit tests:** Fingerprint correct/incorrect ROMs; extract CHR/palette/nametable blobs.
- **Integration test:** Decompile all 22 examples; recompile each; byte-compare ROM.
- **Emulator test:** For each example, run decompile → recompile → emulator harness; assert golden matches.

---

## Milestone 4: FamiTone2 Driver Recognition

**Objective:** Recognize FamiTone2-compatible audio drivers and extract music/SFX data into structured .ne declarations.

### 4.1 FamiTone2 signature recognition (src/decompiler/patterns/famitone.rs)

- Match the standard FamiTone2 init sequence (the first few bytes of code that set up the APU).
- Extract the period table location (usually 60 bytes of lookup table).
- Locate the music play routine and SFX trigger routine.
- Tests: identify FamiTone2 in a real ROM; reject non-FamiTone audio drivers with a diagnostic.

### 4.2 Audio data extraction (src/decompiler/patterns/audio.rs)

Given the driver location:

- **Period table:** Extract 60 entries (u16 each, little-endian), invert to note names (C1-B5). Build a reverse map: period → note index.
- **Music data blocks:** Find music track headers (usually address pointers + metadata), extract note sequences, reconstruct as `music Name { duty, volume, repeat, notes }`.
- **SFX data blocks:** Extract pitch envelope tables and volume envelopes, emit as `sfx Name { duty, pitch: [...], volume: [...] }`.
- Tests: extract period table and invert correctly; music/SFX data round-trips (can be recompiled to identical bytes).

### 4.3 Emitter update (src/decompiler/emitter.rs)

- If FamiTone2 is detected, emit structured music/sfx declarations instead of a `raw_bank`.
- Ensure emitted .ne has the FamiTone2 driver code as a `raw_bank` (unchanged from original).
- Tests: decompiled FamiTone2 ROM compiles and audio hashes match original.

### 4.4 Test coverage for M4

- **Unit tests:** FamiTone2 signature detection; period table inversion; music/SFX data parsing.
- **Integration test:** Decompile audio_demo.ne; assert music/SFX are extracted; recompile and compare audio hash.
- **Emulator test:** audio_demo.ne decompiled → recompiled → goldens match.

---

## Milestone 5: Round-Trip Integration Tests

**Objective:** Formalize the decompile → recompile → golden-diff cycle as an automated test.

### 5.1 New test harness (tests/decompiler_roundtrip.rs)

```rust
#[test]
fn roundtrip_all_examples() {
    for example in examples/*.nes {
        let decompiled = nescript::decompile(example)?;
        let recompiled = nescript::build(decompiled)?;
        let decompiled_rom = std::fs::read(example)?;
        assert_eq!(recompiled, decompiled_rom, "ROM mismatch for {}", example);
    }
}
```

Run in CI: `cargo test --all-targets roundtrip_`.

### 5.2 Emulator golden verification

Extend tests/emulator/run_examples.mjs to support decompiled ROMs:

- Decompile each example → .ne.
- Recompile the decompiled .ne.
- Run the emulator harness on the recompiled ROM.
- Assert the framebuffer PNG and audio hash match the original golden (not just the binary ROM match).

This is the real correctness oracle: if the decompiled ROM renders identically in the emulator, it's correct regardless of byte-level ROM differences.

### 5.3 CI integration

- Add a new CI job `decompile-roundtrip` that runs the harness.
- On failure, upload diffs (actual vs. golden) as artifacts (same as the `emulator` job).
- Add a diagnostic showing which example failed and a hint to re-run locally.

### 5.4 Test coverage for M5

- **Unit test:** Decompile all examples; recompile; byte-compare.
- **Emulator test:** Decompile → recompile → emulator harness for all examples; goldens match.
- **CI:** New job passes on all commits; fails loudly if any example decompile/recompile cycle breaks.

---

## Implementation Order & Dependencies

**Phase 1 (Days 1-4, M1 parallel with prep):**
- M1: Language foundations (parser, analyzer, linker changes)
- Prep: Set up src/decompiler/ module structure

**Phase 2 (Days 5-6, M2 parallel with M3 setup):**
- M2: goto/label escape hatch
- Setup: src/decompiler/ module stubs, main.rs decompile subcommand

**Phase 3 (Days 7-11, M3 + M4 in parallel):**
- M3: decompile identity pass-through + simple asset lifting
- M4: FamiTone2 driver recognition + audio extraction

**Phase 4 (Days 12-14, M5):**
- M5: Round-trip integration tests + CI job

**Phase 5 (Days 15-16, review & cleanup):**
- Code review, fix issues
- Remove decomp-plan.md
- Update docs/future-work.md (mark completed items)
- Write docs/decompiler-guide.md (user-facing decompilation workflow)
- Update README.md to mention decompiler

---

## Success Criteria (Aggregate)

1. ✅ All language features (M1-M2) parse and compile; examples still work.
2. ✅ `nescript decompile <rom>` subcommand exists and produces valid .ne.
3. ✅ Identity round-trip: `decompile x.nes | build` produces ROM byte-identical to x.nes for all examples.
4. ✅ Asset lifting: CHR, palette, nametable, and audio are extracted to structured declarations.
5. ✅ Emulator goldens: Decompiled + recompiled ROMs pass the emulator harness (pixel/audio hashes match).
6. ✅ CI: New `decompile-roundtrip` job passes on all commits.
7. ✅ Documentation: decomp-plan.md removed, docs/decompiler-guide.md added, future-work.md updated.

---

## Known Risks & Mitigations

| Risk | Likelihood | Mitigation |
|------|------------|-----------|
| Pinned address collision undetected | Medium | Analyzer + linker exhaustively check; test with adversarial pinned overlaps |
| goto/label flow analysis incomplete | Low | Conservatively reject gotos in non-unstructured blocks; emulator tests catch codegen bugs |
| FamiTone2 pattern match too narrow | Medium | Start with strict signature match; loosen heuristics if real ROMs miss; document patterns |
| Emulator golden drift | Low | Use existing pinned goldens from repo; only regenerate on intentional audio/render changes |

---

## Out of Scope (Explicitly)

- Full structured lift of arbitrary 6502 code (too hard, beyond the hybrid-shim goal).
- Multi-channel tracker playback runtime (only data extraction for FamiTone2 compat).
- Metatile / tilemap decompilation (defer to post-M5).
- Per-scanline CHR bank switching analysis (defer to post-M5).
- Decompilation of non-NEScript ROMs (fingerprint fails → identity pass-through only).

---

## Commit Strategy

Each milestone should be **one or two commits:**

1. **M1:** Language changes (parser + analyzer + linker, single commit).
2. **M2:** goto/label (parser + analyzer + codegen, single commit).
3. **M3:** decompile subcommand skeleton + identity + simple lifting (two commits: setup, then features).
4. **M4:** FamiTone2 patterns (one or two, depending on complexity).
5. **M5:** Integration tests + CI job (one commit).

Each commit must:
- Pass `cargo fmt --check`.
- Pass `cargo clippy --all-targets -- -D warnings`.
- Pass `cargo test --all-targets`.
- Not break any example ROM reproducibility or emulator golden.

---

## Agents & Parallelization

Recommend parallel agent teams:

- **Agent A (M1):** Parser/analyzer/linker language foundations (4 days). Can start immediately.
- **Agent B (M2, then M3 setup):** goto/label, decompiler module skeleton, main.rs (2+3 days). Depends on M1 completion.
- **Agent C (M3 identity + lifter):** decompile identity pass-through, asset lifting (3 days). Depends on M1.
- **Agent D (M4):** FamiTone2 recognition + audio extraction (3 days). Can start after M3 setup.
- **Agent E (M5):** Integration tests, CI (2 days). Depends on M3, M4.

**Serialization points:**
- M1 complete before A/B/C start in earnest.
- M3 identity working before M4 starts (driver matching assumes known ROM structure).
- All others complete before M5 integration tests.

---

## Document & Tutorial Goals (Post-Milestone)

Once complete, the decompiler should support this workflow:

```bash
# Decompile an existing NEScript-produced ROM
nescript decompile examples/platformer.nes > platformer_mod.ne

# Edit the source
sed -i 's/PLAYER_WALK_SPEED: fixed8.8 = 1.5/PLAYER_WALK_SPEED: fixed8.8 = 2.0/' platformer_mod.ne

# Recompile with custom code
nescript build platformer_mod.ne -o platformer_mod.nes

# Verify in the emulator harness
UPDATE_GOLDENS=0 node tests/emulator/run_examples.mjs  # Compare against original goldens
```

Users should be able to:
1. Decompile any NEScript-produced ROM.
2. Edit constants, assets, audio, and structure.
3. Recompile to a new ROM with changes applied.
4. Rely on the emulator test harness to verify correctness (pixel/audio match).
