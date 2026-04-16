# Performance work

Six performance wins surfaced by the SHA-256 example's inner-loop
analysis. Working through them on this branch; this file is a
scratchpad for the milestone and gets deleted in the final commit
once everything is shipped.

The numbers are per 64-byte SHA-256 block on the example, where
the baseline is ~550K cycles ≈ 18 NTSC frames per block.

## #1 — Skip parameter spill in leaf functions

**Status**: TODO
**Where**: `src/codegen/ir_codegen.rs`
**Estimate**: ~66K cycles/block, ~2.2 frames

Every function currently opens with `LDA $04 / STA <local>` to
spill the param transport slots into a per-function RAM slot,
defending against nested calls that would re-clobber `$04-$07`.
Functions that never `JSR` from inside their body are leaves —
the spill is dead work for them.

Fix: in `IrCodeGen::new`, scan each function for an `IrOp::Call`.
Functions with none get their parameters mapped directly to the
transport slots `$04-$07`, and `gen_function` skips emitting the
spill prologue.

## #2 — Direct-branch comparisons (drop bool materialization)

**Status**: TODO
**Where**: `src/codegen/ir_codegen.rs` (`Lt`/`Gt`/`Eq`/etc. lowering)
**Estimate**: ~9K cycles/block, ~0.3 frames

`if x < N` currently lowers to "compute the result as 0 or 1 in A,
then `BEQ` on it":

```
LDA x          ; 3
CMP #N         ; 2
BCC cmp_t      ; 2/3
LDA #0         ; 2
JMP cmp_e      ; 3
cmp_t: LDA #1  ; 2
cmp_e: BEQ end ; 3
```

The canonical 6502 idiom is one `CMP` + one branch (8 cycles vs.
~16). The IR already gives us the false target label — we just
need to teach `gen_op` for the comparison ops to branch directly
when their result feeds straight into a conditional.

## #3 — Drop dead `LDA #imm` before `INC`/`DEC`

**Status**: TODO
**Where**: `src/codegen/peephole.rs`
**Estimate**: ~5K cycles/block, ~0.2 frames

`i += 1` currently emits:

```
LDA #1           ; 2 — A is overwritten by the next op
INC ZeroPage(i)  ; 5
```

The `LDA #1` is dead. A peephole rule "drop `LDA #imm` if A is
re-written or never read before the next `LDA`/branch/RTS" should
catch this. The same rule fires elsewhere (any `+= const` /
`-= const` that strength-reduced to INC/DEC).

## #4 — Specialize `rotr_wk` per amount (.ne refactor)

**Status**: TODO
**Where**: `examples/sha256/sha_core.ne`
**Estimate**: ~45K cycles/block, ~1.5 frames

`rotr_wk(dst, n)` is a generic loop wrapper. Every SHA-256
rotation amount is a compile-time constant (2, 6, 7, 11, 13, 17,
18, 19, 22, 25), so the loop body is wasted work — the compiler
can't see through the runtime `n`.

Fix: declare one `rotr_wk_<N>(dst)` per used amount, each calling
the appropriate sequence of `byte_rotr_wk` and `rotr1_wk`. The
sigma helpers swap `rotr_wk(SIG, 6)` → `rotr_wk_6(SIG)`. The
loop wrapper stays available for any future caller that needs a
runtime amount.

## #5 — Inline-asm `{param}` substitution after `inline fun` splice

**Status**: DEFERRED
**Where**: `src/ir/lowering.rs::try_inline_call_stmt` +
            `src/codegen/ir_codegen.rs::substitute_asm_vars`
**Estimate**: ~45K cycles/block (potential), ~1.5 frames

Marking primitives `inline fun` would eliminate JSR + RTS + the
rest of the call apparatus — but the inline-asm `{param}`
substitution today resolves names against the analyzer's per-
function allocation table, which doesn't see the inline frame.
A spliced `cp_wk(32, 28)` ends up emitting `LDX {dst}` against
the *caller's* scope where `dst` doesn't exist.

Properly fixing this is non-trivial: substitution needs to be
addressing-mode-aware (immediate `#$20` vs. zero-page `$27` vs.
absolute) and depends on whether the inline arg is a constant or
a runtime value. Documenting the design here so a future pass can
take it.

The simpler half of the win: at inline expansion time, build a
per-frame map from param-name → arg-temp. Pass that map down to
`gen_op`'s asm handler, which substitutes `{name}` with the
arg-temp's allocated slot instead of the param's address. For
constant args, fold further to immediates. This is ~150 lines of
Rust; deferring to a follow-up.

## #6 — Const-fold `r << 2` style index math

**Status**: TODO
**Where**: `src/optimizer/mod.rs`
**Estimate**: <1K cycles/block, negligible

`round_one(r << 2)` recomputes the shift on every iteration of the
phased compression driver. The optimizer already folds shifts when
both operands are constant; extending it to fold "shift a constant
by a constant" inside the IR would catch this case. Trivial in
cycles but worth the cleanliness.

---

## After-each-change checklist

For every codegen change, verify:

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings` on rustc 1.95.0
- [ ] `cargo test --all-targets` passes
- [ ] Rebuild every committed `examples/*.nes`
- [ ] Run the emulator harness — if any golden drifts, eyeball the
      diff and update with `UPDATE_GOLDENS=1`
- [ ] Regenerate `docs/{platformer,war,pong}.gif` if any of those
      examples' captured frames changed

For each `.ne` change:

- [ ] Just the example-side checks (rebuild ROM, harness, gif).
