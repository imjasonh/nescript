# NEScript v0.1 — Compiler Bugs and Limitations Found While Building War

This document captures bugs and limitations discovered while
building `examples/war.ne`. Each entry includes a minimal
reproduction, the symptom we observed, the root cause if known,
and a workaround we used in `examples/war/*.ne`. The intent is
to track these so they can be fixed in a future compiler pass —
once they are, the corresponding workarounds in `war/*.ne`
should be reverted to keep the example honest.

---

## 1. Functions with more than 4 parameters silently corrupt the 5th+

### Symptom

Calling a function with 5 or 6 parameters compiles cleanly, with
no warning or error, but at runtime the 5th and 6th parameter
values are silently replaced by garbage (typically the value of
parameter 3 or 4). Animations and state writes that depend on
those parameters behave as if zero was passed.

### Reproduction

```nescript
fun arm_fly(sx: u8, sy: u8, dxsign: u8, dysign: u8, card: u8, fu: u8) {
    fly_x = sx
    fly_y = sy
    fly_dx_sign = dxsign
    fly_dy_sign = dysign
    fly_card = card        // gets the value of dxsign instead!
    fly_face_up = fu       // gets the value of dxsign instead!
}

fun caller() {
    arm_fly(32, 64, 0, 0, 147, 1)
    // After this call:
    //   fly_x = 32, fly_y = 64, fly_dx_sign = 0, fly_dy_sign = 0
    //   fly_card = 0   (NOT 147)
    //   fly_face_up = 0 (NOT 1)
}
```

### Root cause

`src/codegen/ir_codegen.rs` (around line 240) iterates through
`func.locals` and assigns the first 4 entries to zero-page
parameter slots `$04`-`$07`:

```rust
for func in &ir.functions {
    for (i, local) in func.locals.iter().enumerate() {
        if i < func.param_count {
            if i < 4 {
                var_addrs.insert(local.var_id, 0x04 + i as u16);
                ...
            }
        } else {
            ...
        }
    }
}
```

The `if i < 4` guard silently drops the mapping for params 5+
without inserting any RAM allocation for them. The corresponding
caller-side codegen for `Call` writes only the first four
arguments. Result: params 5 and 6 are never passed and the
callee reads stale memory from $04-$07 in their place.

### Workaround used in `examples/war/`

`arm_fly` is split: the four "arming" parameters stay in the
function signature, and `fly_card` / `fly_face_up` are written to
the global state directly at every call site instead. See
`war/play_state.ne` (`begin_draw_a` / `begin_draw_b`).

### Fix proposal

Two reasonable options:

1. **Diagnose-only**: emit `E05XX too many parameters` when a
   `fun` declaration has more than 4 params. This is the
   smallest possible change and turns silent miscompiles into a
   loud compile-time error. Should ship immediately even if
   option 2 is also planned.

2. **Spill to RAM**: extend the calling convention so params
   beyond the first four are passed via dedicated RAM slots in
   the callee's local frame. The caller-side `Call` codegen
   would write those slots before `JSR`, the callee-side prologue
   could leave them as-is. This grows the per-function RAM
   footprint but lets users write any signature they like.

---

## 1b. Function parameters with the same name in different functions share a VarId, which collides their zero-page slot mapping

### Symptom

Two unrelated functions whose parameters happen to be named the
same (e.g. both have a `card: u8` parameter, or both have an
`x: u8` parameter) end up reading parameters from the wrong
zero-page slot at runtime. One function reads `$04`, another
reads `$06`, a third reads `$05` — depending on the parameter's
*position* in whichever function is processed last by the
codegen.

This is a much sneakier sibling of bug #1: rather than dropping
a parameter past the 4th slot, it silently reroutes parameter
reads to slots that hold completely unrelated values from the
caller.

### Reproduction

```nescript
// Function A: card is the 1st parameter, expected at $04
fun push_back_a(card: u8) {
    deck_a[deck_a_front] = card   // reads from $06, not $04!
    deck_a_count += 1
}

// Function B: card is the 3rd parameter, expected at $06
fun draw_card_face(x: u8, y: u8, card: u8) {
    // ... uses card normally ...
}
```

The IR lowering assigns `card` a single shared `VarId` because
its `var_map` is global across all functions. The codegen then
walks each function in turn, inserting `(VarId(card), $0X)`
mappings into a single global `var_addrs` `HashMap` — and
whichever function comes last in iteration order wins the
mapping. If `draw_card_face` is processed after `push_back_a`,
`VarId(card)` ends up mapped to `$06`, and `push_back_a` then
reads its `card` parameter from `$06` (which holds whatever the
caller was using as a third argument — typically junk).

### Root cause

`src/ir/lowering.rs::get_or_create_var` looks up names in
`self.var_map`, which is shared across the whole program:

```rust
fn get_or_create_var(&mut self, name: &str) -> VarId {
    if let Some(&id) = self.var_map.get(name) {
        id
    } else {
        let id = VarId(self.next_var_id);
        self.next_var_id += 1;
        self.var_map.insert(name.to_string(), id);
        id
    }
}
```

`lower_function` calls `get_or_create_var(&param.name)` for each
parameter, so two different functions both with a `card`
parameter resolve to the same `VarId`. Once that single `VarId`
flows into the codegen, the per-function "this is param index N
of function F" relationship is lost — there's only one global
mapping per `VarId`.

### Workaround used in `examples/war/`

Every parameter name in the war source is unique across the
entire program. Function-locals were already prefixed by
function (see bug #3); we extended the same scheme to params:
`push_back_a(pba_arg_card: u8)` instead of
`push_back_a(card: u8)`, etc. The wrapping `pba_card` /
`pbb_card` / `dcf_card` snapshots from bug #2 stay because they
also help with the bug-2 clobbering.

### Fix proposal

Two layers to fix in:

1. **IR lowering**: give every function its own `var_map` for
   parameters and locals. The global `var_map` should only hold
   top-level `var` / `const` / `enum` symbols.

2. **Codegen**: even after the IR fix, the global `var_addrs`
   `HashMap` should grow a per-function dimension (one map per
   `IrFunction`) so two different functions can independently
   assign their own VarIds to overlapping zero-page slots.

Either fix alone is probably enough; both together is robust.

---

## 2. Function parameters share zero-page slots with nested calls — values clobbered across `JSR`

### Symptom

A function that takes parameters and then calls another function
sees its own parameters silently replaced by the inner call's
arguments. Any code path that reads the original parameter
*after* the inner call gets the wrong value.

### Reproduction

```nescript
fun draw_card_face(x: u8, y: u8, card: u8) {
    var rank: u8 = card_rank(card)   // x at $04 is now `card`
    var suit: u8 = card_suit(card)   // x at $04 is still `card`
    // x is supposed to be 120 here, but it's actually `card`
    var x1: u8 = x + 8               // computes card + 8, not 120 + 8
    draw Tileset at: (x, y) frame: ...   // draws at x = card, not 120
}
```

Concretely, calling `draw_card_face(120, 128, 0x93)` puts the
card sprite at `(0x93, 128)` — completely wrong.

### Root cause

Same allocator as bug #1: `func.locals[0..param_count]` are
mapped to `$04`, `$05`, `$06`, `$07`. The caller writes its own
arguments into the same zero-page slots before `JSR`, so the
caller's parameters at those slots get clobbered by the callee's
arguments. There is no save/restore wrapper around `JSR` and no
spill/reload pass to refresh the caller's parameters from a
backing copy.

### Workaround used in `examples/war/`

Every helper that takes parameters AND makes any nested function
call snapshots its parameters into fresh local variables at the
top of the function, then references the locals exclusively
throughout the body. See `war/render.ne::draw_card_face`,
`war/render.ne::draw_flying_card`, `war/deck.ne::push_back_a`,
`war/deck.ne::push_back_b`.

### Fix proposal

1. **Spill on entry**: at the top of every function body that
   makes a call, copy `$04..$07` into per-function RAM slots and
   rewrite all parameter reads to load from the RAM copies.
   Equivalent to what users are doing manually today.

2. **Smarter scheduling**: only spill a parameter slot if it's
   live across a call site (CFG-aware liveness pass on params).
   Same effect, less RAM cost for short helpers that never read
   their params after calling out.

Either fix would let users write straightforward function bodies
without having to remember the snapshot dance.

---

## 3. Function-local variable names are in a flat global namespace

### Symptom

Two different functions cannot declare locals with the same
name. The compiler emits `E0501 duplicate declaration of '<name>'`
even though the locals are in disjoint scopes.

### Reproduction

```nescript
fun foo() {
    var i: u8 = 0
    while i < 10 { i += 1 }
}

fun bar() {
    var i: u8 = 0   // E0501 duplicate declaration of 'i'
    while i < 5 { i += 1 }
}
```

### Root cause

`src/analyzer/mod.rs::register_var` inserts every `var`
declaration into a single `self.symbols` map keyed only on the
variable's name, with no qualification by function or block:

```rust
fn register_var(&mut self, var: &VarDecl) {
    if self.symbols.contains_key(&var.name) {
        self.diagnostics.push(Diagnostic::error(
            ErrorCode::E0501,
            format!("duplicate declaration of '{}'", var.name),
            var.span,
        ));
        return;
    }
    ...
}
```

`check_statement` calls `register_var` for every `Statement::VarDecl`
encountered while walking function bodies, so all locals across
all functions and all nested blocks land in the same namespace.

### Workaround used in `examples/war/`

Every function-local variable is prefixed with a short tag
identifying its enclosing function (e.g. `dfa_card` in
`draw_front_a`, `pba_slot` in `push_back_a`,
`dwp_px` in `draw_word_player`). This makes long files harder to
read but is fully mechanical.

### Fix proposal

Rework `register_var` to maintain a stack of scopes (one per
function body, one per nested block). Each `Statement::VarDecl`
inserts into the current scope. Lookup walks the stack from
innermost to outermost. The existing global symbol table is
unchanged for top-level globals / consts / fun names; only
function-locals shift to the scoped table.

A smaller intermediate fix: keep the flat table but qualify
each local's stored name as `<function>::<var>` so the global
table sees unique entries even when source names collide.

---

## 4. Per-frame sprite-per-scanline limit is invisible to user code

### Symptom

Drawing more than 8 sprites whose Y rectangles intersect a
single scanline causes the NES PPU to silently drop the excess
sprites past the 8th in OAM order. There's no compile-time
detection and no runtime warning — letters or tiles just don't
render.

### Reproduction

```nescript
// 9 letters all on the same Y row:
draw_letter(0,   100, 0)
draw_letter(8,   100, 1)
draw_letter(16,  100, 2)
draw_letter(24,  100, 3)
draw_letter(32,  100, 4)
draw_letter(40,  100, 5)
draw_letter(48,  100, 6)
draw_letter(56,  100, 7)
draw_letter(64,  100, 8)   // this one will not render
```

### Root cause

This is a real NES hardware constraint, not a compiler bug.
However, because NEScript's `draw` allocator is purely
sequential, the compiler cannot warn even when it has all the
information needed to know the layout would overflow.

### Workaround used in `examples/war/`

We staggered text rows. The title screen's "WAR / CARD GAME /
0 PLAYER / 1 PLAYER / 2 PLAYER" layout sits each row at a
different y so no scanline carries more than 7 sprites; the
victory screen's "PLAYER X / WINS" wraps after the player letter
for the same reason.

### Fix proposal

Two complementary improvements:

1. **Static analyzer pass**: walk the IR for each frame handler,
   collect the set of `(x, y)` literal pairs feeding `draw`
   ops within the same basic block, and emit `W01XX` if any
   scanline (8-px row) would have > 8 sprites. Only catches the
   literal case but that's the most common.

2. **Sprite-cycling runtime helper**: a `cycle_sprites()`
   intrinsic that rotates OAM order each frame so the same
   sprites get dropped on different frames, producing a flicker
   instead of a permanent dropout. Standard NES technique.

---

## 5. The `inline` keyword is a hint and is silently ignored for short functions

### Symptom

Marking a tiny function `inline fun` does not always inline it.
The compiler still emits a real `JSR` with full parameter
passing through `$04`-`$07`, which means the inlining doesn't
escape the bug-2 parameter clobbering.

### Reproduction

```nescript
inline fun card_rank(card: u8) -> u8 {
    return card >> 4
}
```

The asm dump shows `JSR __ir_fn_card_rank` at every call site —
the function was not inlined.

### Root cause

(Inferred — would need to confirm by reading the inliner pass.)
The optimizer's inlining pass has a size threshold or a heuristic
that prevents inlining in some contexts even when the function
is marked `inline`. There's no diagnostic emitted when the hint
is declined.

### Workaround used in `examples/war/`

None — we just live with the JSR overhead and the bug-2 fallout.

### Fix proposal

1. **Promote `inline` to a hard contract**: when `inline` is
   present, always inline (or emit `W01XX` if it cannot be
   inlined for a structural reason like recursion).

2. **Optional dump**: add `--dump-inliner` to print which
   `inline fun` declarations were inlined and which weren't,
   with the reason.

---

## 6. `wide_hi` IR-lowering map leaked between functions and corrupted 16-bit ops *(FIXED)*

### Symptom

A function whose body had no 16-bit values whatsoever would
nonetheless emit `CmpEq16` (and other `Op16` variants) where the
*destination* temp aliased one of the *source* temps. The
resulting comparison effectively became "is this byte equal to
some uninitialised stack memory?", which in War caused the
phase-machine `match phase { ... }` dispatcher to skip the
`P_WIN_B` arm forever once the game first reached it — the game
would freeze with both cards face-up and "PLAYER B WINS" never
firing.

### Reproduction (pre-fix)

A handful of `u16` `+= 1` operations early in a state handler
followed by a long `match` chain on a `u8` was enough to trip it.
The minimum repro is roughly:

```nescript
var clock: u16 = 0
var phase: u8 = 0
on frame {
    clock += 1                    // wide op leaves wide_hi entries
    match phase {                 // u8 match — should be 8-bit
        0 => { phase = 1 }
        1 => { phase = 2 }
        2 => { phase = 3 }
        3 => { phase = 4 }
        4 => { phase = 5 }
        5 => { phase = 6 }
        6 => { phase = 7 }
        7 => { /* corrupt — never matched */ }
        _ => {}
    }
}
```

The IR for the `phase == 7` arm came out as
`CmpEq16 { dest: T147, a_lo: T145, a_hi: T148, b_lo: T146,
b_hi: T147 }` — note `dest == b_hi`. The codegen happily emits
the corresponding 16-bit asm, but reads garbage for the `b_hi`
operand because it points at the same scratch slot the result
will be written to.

### Root cause

`src/ir/lowering.rs::IrLowerer` carries a `wide_hi: HashMap<IrTemp, IrTemp>`
that records "this low temp's high byte lives at this other
temp" pairs whenever a 16-bit value is produced. `lower_function`
and `lower_handler` both reset `next_temp = 0` at the start of
each function — but they did *not* clear `wide_hi`. Stale entries
from earlier functions stuck around and matched against fresh
temp IDs in subsequent functions (which start counting from 0
again), causing `is_wide(t)` and `widen(t)` to return spurious
"wide" results for what should have been narrow `u8` values.

When that happens inside `lower_binop`'s `Eq` path, `widen(r)`
returns the stale `(r, hi_r)` pair where `hi_r` happens to be the
*next* temp ID `fresh_temp()` will hand out a moment later — so
the `dest` temp and `b_hi` end up identical.

### Fix

`src/ir/lowering.rs`: in both `lower_function` and `lower_handler`,
add `self.wide_hi.clear();` immediately after `self.next_temp = 0;`.
Done in this PR.

### Why this didn't show up sooner

Every prior example either declared no `u16` globals at all, or
declared one and used it sparingly enough that the temp IDs
the leaked entries claimed never collided with the rest of the
function. War is the first example that combines a `u16`
free-running counter with a deep state machine that does many
`u8` comparisons in the same `on frame` body, which is exactly
the shape the bug needs to manifest.

### Regression test

`src/ir/tests.rs::wide_hi_does_not_leak_between_functions` (added
in this PR) compiles a two-function program where function A
uses a `u16 += 1` (creating wide entries) and function B does
`u8 == const` comparisons in a match. Pre-fix, the IR would emit
`CmpEq16` with aliased dest/source; post-fix it emits the
expected 8-bit `CmpEq`.

---

## Verification path after fixes

Once any of the bugs above are fixed in the compiler, the
corresponding workarounds in `examples/war/*.ne` should be
reverted in the same PR so:

- The example demonstrates idiomatic code, not workaround code.
- The PR's diff visibly proves the fix works end-to-end (the
  workaround removal would otherwise be a silent regression).
- The committed `examples/war.nes` rebuilds byte-identically to
  the reverted source, which the pre-commit hook enforces.

The relevant workaround sites are catalogued in each bug's
"Workaround used" section above; grep for the prefix tags
(`dcf_`, `dfa_`, `pba_`, `dwp_`, …) to find them all.
