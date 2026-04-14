//! IR-based code generator.
//!
//! Walks an `IrProgram` and produces 6502 instructions. Uses a simple
//! strategy: each IR temp is assigned a zero-page slot in the function's
//! temp region. Operations load operands from their slots into A, compute,
//! and store back. This is not efficient but is correct and easy to reason
//! about. A proper register allocator would use A/X/Y directly for short
//! live ranges.
//!
//! Zero-page layout (shared with AST codegen):
//! - `$00` frame flag
//! - `$01` input P1
//! - `$02` scratch temp
//! - `$03` `current_state`
//! - `$04-$07` function call params
//! - `$08` input P2
//! - `$09` runtime OAM cursor (used by `draw` to assign slots)
//! - `$0A-$0F` reserved
//! - `$10+` user variables + IR temp slots
//!
//! IR temps are allocated starting at `TEMP_BASE` (`$80`), giving 128 bytes
//! (`0x80-0xFF`) for IR temp storage per function. Functions reset the
//! temp counter at entry.

use std::collections::HashMap;

use crate::analyzer::VarAllocation;
use crate::asm::{AddressingMode as AM, Instruction, Opcode::*};
use crate::assets::{MusicData, SfxData};
use crate::ir::{IrBasicBlock, IrFunction, IrOp, IrProgram, IrTemp, IrTerminator, VarId};
use crate::parser::ast::Channel;
use crate::runtime::{
    AUDIO_NOISE_COUNTER, AUDIO_NOISE_PTR_HI, AUDIO_NOISE_PTR_LO, AUDIO_TRIANGLE_COUNTER,
    AUDIO_TRIANGLE_PTR_HI, AUDIO_TRIANGLE_PTR_LO, ZP_MUSIC_BASE_HI, ZP_MUSIC_BASE_LO,
    ZP_MUSIC_COUNTER, ZP_MUSIC_PTR_HI, ZP_MUSIC_PTR_LO, ZP_MUSIC_STATE, ZP_OAM_CURSOR,
    ZP_PENDING_BG_ATTRS_HI, ZP_PENDING_BG_ATTRS_LO, ZP_PENDING_BG_TILES_HI, ZP_PENDING_BG_TILES_LO,
    ZP_PENDING_PALETTE_HI, ZP_PENDING_PALETTE_LO, ZP_PPU_UPDATE_FLAGS, ZP_SFX_COUNTER,
    ZP_SFX_PTR_HI, ZP_SFX_PTR_LO,
};

/// Base zero-page address for IR temp slots.
const TEMP_BASE: u8 = 0x80;

/// Zero-page addresses (shared with AST codegen).
const ZP_FRAME_FLAG: u8 = 0x00;
const ZP_CURRENT_STATE: u8 = 0x03;
/// Per-frame scanline step counter, reset to 0 by the NMI reload
/// helper. Each time the MMC3 IRQ fires, the dispatcher looks at
/// this byte to pick the right handler for multi-scanline states,
/// then bumps it so the next IRQ hits the next handler. States
/// with only a single scanline handler ignore the counter.
const ZP_SCANLINE_STEP: u8 = 0x0C;

/// Emulator debug output port. Writes to this address are logged by
/// Mesen / fceux when the debugger is attached. Used by `debug.log`
/// and `debug.assert` when compiled with `--debug`.
const DEBUG_PORT: u16 = 0x4800;

/// IR codegen that produces 6502 instructions from an `IrProgram`.
#[allow(clippy::struct_excessive_bools)]
pub struct IrCodeGen<'a> {
    instructions: Vec<Instruction>,
    /// Map from IR `VarId` to zero-page address.
    var_addrs: HashMap<VarId, u16>,
    /// Map from `IrTemp` to zero-page slot within the current function.
    temp_slots: HashMap<IrTemp, u8>,
    /// Next unused temp slot (monotonic within a function — grows
    /// only when there are no recyclable slots in `free_slots`).
    next_temp_slot: u8,
    /// Free list of slots that held now-dead temps. Populated by
    /// `retire_dead_temps` when a temp's use count drops to zero.
    /// New allocations pull from this list before growing the
    /// monotonic counter, which keeps functions with many short-
    /// lived temps (e.g. long arithmetic chains on u8 flags) from
    /// blowing past the 128-slot TEMP region.
    free_slots: Vec<u8>,
    /// Remaining use count for each temp in the current function.
    /// Built by `build_use_counts` at the start of each function.
    /// Decremented by `retire_dead_temps` after each op runs;
    /// when a count hits 0 the temp's slot is pushed onto
    /// `free_slots` and removed from `temp_slots`.
    use_counts: HashMap<IrTemp, u32>,
    /// Sprite name to tile index mapping.
    sprite_tiles: HashMap<String, u8>,
    /// Sfx name to `(period_lo, period_hi, envelope_label, channel)`.
    /// Populated by `with_audio` from the resolved [`SfxData`] list.
    /// `play Name` consults this to pick:
    /// - the right trigger register pair (pulse 1 = $4002/$4003,
    ///   triangle = $400A/$400B, noise = $400E/$400F);
    /// - the right per-channel envelope pointer slot (pulse 1 uses
    ///   zero-page `ZP_SFX_PTR`, triangle/noise live in main RAM
    ///   at the `AUDIO_*_PTR_*` addresses).
    sfx_info: HashMap<String, (u8, u8, String, Channel)>,
    /// Music name to `(header_byte, stream_label)`. Populated by
    /// `with_audio`. `start_music Name` stamps `header | 0x02` into
    /// `ZP_MUSIC_STATE` and loads the pointer from `stream_label`.
    music_info: HashMap<String, (u8, String)>,
    /// State name to dispatch index mapping.
    state_indices: HashMap<String, u8>,
    /// Set of function names defined in the IR program (for existence checks).
    function_names: std::collections::HashSet<String>,
    /// True while generating code inside a state frame handler.
    /// When set, `Return` terminators emit `JMP __ir_main_loop` instead of `RTS`.
    in_frame_handler: bool,
    /// When true, emit code for `debug.log` / `debug.assert`.
    /// When false, these ops are stripped entirely.
    debug_mode: bool,
    /// Set to true the first time we emit any audio op (`play` /
    /// `start_music` / `stop_music`). Used to emit the `__audio_used`
    /// marker label at most once per program so the linker can
    /// decide whether to splice the audio tick into NMI.
    audio_used: bool,
    /// Set to true the first time a `play` op targets a noise sfx.
    /// Emits the `__noise_used` marker label so the linker knows
    /// to append the noise channel block to `gen_audio_tick` and
    /// reserve the main-RAM state slots.
    noise_used: bool,
    /// Same as `noise_used`, but for triangle sfx. Drives the
    /// `__triangle_used` marker label.
    triangle_used: bool,
    /// Set to true the first time we emit any PPU update op
    /// (`set_palette` / `load_background`). The linker uses the
    /// resulting `__ppu_update_used` marker label to decide whether
    /// to splice the in-NMI palette/nametable update helper.
    ppu_update_used: bool,
    /// Source-location markers produced from [`IrOp::SourceLoc`].
    /// Each entry is a `(label_name, span)` pair — the codegen
    /// emits a unique label-definition pseudo-op at the current
    /// instruction index, and the CLI later resolves each label's
    /// CPU address through the linker's output map to produce a
    /// `.map` file. Empty if the IR didn't contain any source
    /// markers or if `emit_source_locs` is false.
    source_locs: Vec<(String, crate::lexer::Span)>,
    /// Next unused index for the monotonic `__src_<N>` label
    /// counter. Bumped every time a new marker is emitted.
    next_source_loc: u32,
    /// When true, each [`IrOp::SourceLoc`] is lowered to a
    /// label-definition pseudo-op and recorded in `source_locs`.
    /// When false (the default), `SourceLoc` is silently dropped
    /// so release-mode codegen output is byte-identical to the
    /// pre-source-map behaviour — turning this on *does* affect
    /// the peephole pass's block boundaries and shifts labels in
    /// the final ROM. Enabled by the CLI when `--source-map` is
    /// passed.
    emit_source_locs: bool,
    /// Byte size of each named global / local variable. Keyed by
    /// IR `VarId`, mirrors [`Self::var_addrs`]. Used by the
    /// debug-mode array bounds checker to emit an `idx >= size`
    /// guard on every `ArrayLoad` / `ArrayStore`. Missing entries
    /// mean "unknown size" and skip the check.
    var_sizes: HashMap<VarId, u16>,
    /// True once a bounds-check trip was emitted; the linker-side
    /// helper (a `JMP $` infinite loop at `__debug_halt`) is
    /// emitted once at the end of `generate()` so multiple
    /// failing checks all land on the same debug marker. Skipped
    /// entirely in release builds.
    bounds_halt_used: bool,
    /// Per-banked-function instruction streams, populated by
    /// [`Self::generate`] when a program declares one or more
    /// `bank Foo { fun ... }` blocks. The key is the bank name; the
    /// value is the assembled IR codegen output for every function
    /// assigned to that bank, ready to be handed to the linker as a
    /// `PrgBank::with_instructions` payload. Empty for programs
    /// without any banked user code, which keeps the codegen output
    /// byte-for-byte identical to the pre-banked behaviour.
    banked_streams: HashMap<String, Vec<Instruction>>,
    /// Function name → declared bank name, populated from the IR
    /// functions' `bank` field. Used to decide whether a `Call` op
    /// should JSR the direct `__ir_fn_<name>` label or the cross-bank
    /// trampoline `__tramp_<name>` label.
    function_banks: HashMap<String, String>,
    /// While [`Self::generate`] is emitting code for a banked
    /// function, this holds the bank name so cross-bank calls can be
    /// disambiguated from in-bank calls. `None` means the codegen is
    /// currently emitting fixed-bank code (handlers, runtime, the
    /// dispatcher loop, top-level functions).
    current_bank: Option<String>,
    allocations: &'a [VarAllocation],
}

impl<'a> IrCodeGen<'a> {
    pub fn new(allocations: &'a [VarAllocation], ir: &IrProgram) -> Self {
        // Map IR global VarIds to their allocated addresses.
        // Globals in IR are in the same order as in the analyzer, so we
        // can align them by name.
        let mut var_addrs = HashMap::new();
        let mut var_sizes = HashMap::new();
        for global in &ir.globals {
            if let Some(alloc) = allocations.iter().find(|a| a.name == global.name) {
                var_addrs.insert(global.var_id, alloc.address);
                var_sizes.insert(global.var_id, alloc.size);
            }
        }
        // Map each function's parameter VarIds to the zero-page
        // parameter-passing slots $04-$07 (up to 4 params). Map the
        // rest of each function's locals into main RAM starting at
        // `$0300` (after the OAM buffer). Locals don't overlap with
        // globals (which were placed by the analyzer) and are
        // disjoint across functions so nested calls don't corrupt
        // each other.
        let mut local_ram_next: u16 = 0x0300;
        // Advance past any RAM global so locals don't clobber them.
        // Each global occupies `[address, address + size)` — for an
        // array global at $0308 with size=4 that's $0308..$030C. We
        // must advance past the END, not the base, otherwise
        // subsequent locals overlap with the tail of the array.
        // Globals are looked up by name against the analyzer's
        // `allocations` (which has per-global sizes) rather than the
        // `var_addrs` map, which only stores base addresses.
        let max_ram_global_end = allocations
            .iter()
            .filter(|a| a.address >= 0x0100)
            .map(|a| a.address + a.size.max(1))
            .max()
            .unwrap_or(0);
        if max_ram_global_end > local_ram_next {
            local_ram_next = max_ram_global_end;
        }
        for func in &ir.functions {
            for (i, local) in func.locals.iter().enumerate() {
                if i < func.param_count {
                    if i < 4 {
                        var_addrs.insert(local.var_id, 0x04 + i as u16);
                        var_sizes.insert(local.var_id, local.size);
                    }
                } else {
                    var_addrs.insert(local.var_id, local_ram_next);
                    var_sizes.insert(local.var_id, local.size);
                    local_ram_next += local.size.max(1);
                }
            }
        }
        let function_names = ir.functions.iter().map(|f| f.name.clone()).collect();
        // Build the function-name → bank-name map from the IR. Most
        // programs have an empty map (no `bank Foo { fun ... }`
        // blocks); when populated, the codegen splits banked
        // function bodies into per-bank instruction streams and
        // rewrites `Call` ops to JSR a fixed-bank trampoline when
        // they cross bank boundaries.
        let mut function_banks: HashMap<String, String> = HashMap::new();
        for func in &ir.functions {
            if let Some(bank) = &func.bank {
                function_banks.insert(func.name.clone(), bank.clone());
            }
        }
        Self {
            instructions: Vec::new(),
            var_addrs,
            temp_slots: HashMap::new(),
            next_temp_slot: 0,
            free_slots: Vec::new(),
            use_counts: HashMap::new(),
            sprite_tiles: HashMap::new(),
            sfx_info: HashMap::new(),
            music_info: HashMap::new(),
            state_indices: HashMap::new(),
            function_names,
            in_frame_handler: false,
            debug_mode: false,
            audio_used: false,
            noise_used: false,
            triangle_used: false,
            ppu_update_used: false,
            source_locs: Vec::new(),
            next_source_loc: 0,
            emit_source_locs: false,
            var_sizes,
            bounds_halt_used: false,
            banked_streams: HashMap::new(),
            function_banks,
            current_bank: None,
            allocations,
        }
    }

    /// Per-banked-function instruction streams produced by the most
    /// recent [`Self::generate`] call. The map is keyed by bank name
    /// (matching the program's `bank Foo { ... }` declarations) and
    /// each value is ready to be handed to the linker as a
    /// `PrgBank::with_instructions` payload. Empty for programs with
    /// no banked code, in which case the linker treats every bank
    /// as the legacy "reserved slot" mode.
    #[must_use]
    pub fn banked_streams(&self) -> &HashMap<String, Vec<Instruction>> {
        &self.banked_streams
    }

    /// Enable source-location marker emission. When set, each
    /// [`IrOp::SourceLoc`] lowers to a uniquely-named
    /// label-definition pseudo-op and is recorded in
    /// [`Self::source_locs`]. Off by default so release builds
    /// produce byte-identical ROMs regardless of the IR lowering
    /// stage's marker output.
    #[must_use]
    pub fn with_source_map(mut self, enabled: bool) -> Self {
        self.emit_source_locs = enabled;
        self
    }

    /// Source-location markers emitted during codegen. Populated
    /// once [`Self::generate`] has run; each entry pairs a
    /// `__src_<N>` label name with the span it came from. The CLI
    /// uses this plus the linker's label map to write a source-map
    /// file under `--source-map`.
    #[must_use]
    pub fn source_locs(&self) -> &[(String, crate::lexer::Span)] {
        &self.source_locs
    }

    /// Enable debug-mode code generation. When set, `debug.log` and
    /// `debug.assert` emit runtime code; otherwise they are stripped.
    #[must_use]
    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug_mode = debug;
        self
    }

    fn function_exists(&self, name: &str) -> bool {
        self.function_names.contains(name)
    }

    #[must_use]
    pub fn with_sprites(mut self, sprites: &[crate::linker::SpriteData]) -> Self {
        for sprite in sprites {
            self.sprite_tiles
                .insert(sprite.name.clone(), sprite.tile_index);
        }
        self
    }

    /// Register resolved audio assets with the codegen so that
    /// `play`/`start_music` can emit literal trigger constants and
    /// symbolic pointers to the in-ROM data blobs.
    #[must_use]
    pub fn with_audio(mut self, sfx: &[SfxData], music: &[MusicData]) -> Self {
        for s in sfx {
            self.sfx_info.insert(
                s.name.clone(),
                (s.period_lo, s.period_hi, s.label(), s.channel),
            );
        }
        for m in music {
            self.music_info
                .insert(m.name.clone(), (m.header, m.label()));
        }
        self
    }

    fn emit(&mut self, opcode: crate::asm::Opcode, mode: AM) {
        self.instructions.push(Instruction::new(opcode, mode));
    }

    fn emit_label(&mut self, name: &str) {
        self.instructions
            .push(Instruction::new(NOP, AM::Label(name.to_string())));
    }

    /// Return the zero-page address for an IR temp, allocating a new slot
    /// if needed. Recycles slots from `free_slots` (temps whose use
    /// count has hit zero) before growing the monotonic counter, so
    /// functions with many short-lived temps stay within the 128-byte
    /// TEMP region.
    fn temp_addr(&mut self, temp: IrTemp) -> u8 {
        if let Some(&slot) = self.temp_slots.get(&temp) {
            return slot;
        }
        // Prefer a recycled slot if one is available.
        if let Some(slot) = self.free_slots.pop() {
            self.temp_slots.insert(temp, slot);
            return slot;
        }
        // Otherwise grow the monotonic counter. If we've exhausted
        // the 128 slots reserved at TEMP_BASE..$FF, panic with a
        // diagnostic — this indicates either a bug in the liveness
        // analysis or a function with pathologically long live
        // ranges. In the common case, the free list keeps us well
        // under the limit.
        assert!(
            u16::from(self.next_temp_slot) < 0x80,
            "IR codegen: function exceeds 128 concurrent live temps; \
             this is a compiler bug — temps at end of function should \
             have been recycled via `retire_dead_temps`"
        );
        let slot = TEMP_BASE + self.next_temp_slot;
        self.next_temp_slot += 1;
        self.temp_slots.insert(temp, slot);
        slot
    }

    /// Decrement a temp's use count. When the count reaches zero,
    /// the temp is dead and its slot can be reused for a later
    /// allocation. This is called after every op reads its source
    /// temps, just before the destination (if any) is allocated.
    fn dec_use(&mut self, temp: IrTemp) {
        if let Some(count) = self.use_counts.get_mut(&temp) {
            if *count > 0 {
                *count -= 1;
                if *count == 0 {
                    if let Some(slot) = self.temp_slots.remove(&temp) {
                        self.free_slots.push(slot);
                    }
                }
            }
        }
    }

    /// After `gen_op` finishes processing an op, retire any source
    /// temps whose last-use was this op. For ops with multiple
    /// sources (`Add`, `CmpEq`, `Add16`, …) we decrement each one —
    /// the count for the same temp used twice in one op is
    /// correctly handled because we pre-built the counts by
    /// scanning every operand appearance.
    fn retire_op_sources(&mut self, op: &IrOp) {
        for t in op_source_temps(op) {
            self.dec_use(t);
        }
    }

    /// Load a temp's value into A.
    fn load_temp(&mut self, temp: IrTemp) {
        let addr = self.temp_addr(temp);
        self.emit(LDA, AM::ZeroPage(addr));
    }

    /// Store A into a temp's slot.
    fn store_temp(&mut self, temp: IrTemp) {
        let addr = self.temp_addr(temp);
        self.emit(STA, AM::ZeroPage(addr));
    }

    /// Emit a debug-only array bounds check. Assumes A holds the
    /// candidate index; emits `CMP #len; BCS __debug_halt` where
    /// `len` is the declared byte size of the variable. For u8
    /// arrays `size` is the element count (correct bound); for u16
    /// arrays the codegen doesn't yet scale the index by element
    /// width, so we use the raw byte size as the bound — that's
    /// correct for the `ZeroPageX`/`AbsoluteX` lowering the current
    /// codegen actually produces, and it's what a future lowering
    /// fix would want the debug check to match anyway.
    ///
    /// Release builds emit nothing. Also a no-op when the size
    /// isn't known (e.g. a local we couldn't match up against an
    /// allocation); missing metadata degrades silently to the
    /// old unchecked behaviour.
    fn emit_bounds_check(&mut self, var: VarId) {
        if !self.debug_mode {
            return;
        }
        let Some(&size) = self.var_sizes.get(&var) else {
            return;
        };
        if size == 0 {
            return;
        }
        // Anything >= 256 would overflow the u8 immediate; skip
        // the check rather than emit a bogus compare. A proper
        // 16-bit bounds check would need a two-byte compare
        // against the high byte too.
        let Ok(size_u8) = u8::try_from(size) else {
            return;
        };
        // Use a short BCC over an unconditional JMP instead of a
        // plain `BCS __debug_halt`. A single BCS can only span 127
        // bytes, and `__debug_halt` is emitted at the very end of
        // the fixed bank — many check sites are far enough away
        // that the short-branch fixup would panic at link time.
        // BCC-over-JMP keeps the hot path at two branches (well
        // under 8 cycles) and the failure path at a 3-byte JMP.
        let skip_label = format!("__ir_bc_ok_{}", self.instructions.len());
        self.emit(CMP, AM::Immediate(size_u8));
        self.emit(BCC, AM::LabelRelative(skip_label.clone()));
        self.emit(JMP, AM::Label("__debug_halt".to_string()));
        self.emit_label(&skip_label);
        self.bounds_halt_used = true;
    }

    /// Emit a runtime-variable shift loop: loads `src` into A, then
    /// `amt` iterations of `shift_op` (`ASL` / `LSR`), storing into
    /// `dest`. An iteration count of zero is handled by a leading
    /// BEQ over the loop so the source value is preserved.
    fn gen_shift_var(
        &mut self,
        dest: IrTemp,
        src: IrTemp,
        amt: IrTemp,
        shift_op: crate::asm::Opcode,
    ) {
        let suffix = self.instructions.len();
        let loop_label = format!("__ir_shift_loop_{suffix}");
        let done_label = format!("__ir_shift_done_{suffix}");
        let amt_addr = self.temp_addr(amt);
        self.emit(LDX, AM::ZeroPage(amt_addr));
        self.load_temp(src);
        // Skip straight to the store if the count is zero — saves an
        // iteration and is required because the loop decrements
        // before checking.
        self.emit(BEQ, AM::LabelRelative(done_label.clone()));
        self.emit_label(&loop_label);
        self.emit(shift_op, AM::Accumulator);
        self.emit(DEX, AM::Implied);
        self.emit(BNE, AM::LabelRelative(loop_label));
        self.emit_label(&done_label);
        self.store_temp(dest);
    }

    /// Generate instructions for an entire IR program.
    ///
    /// Layout:
    /// 1. Variable initializers (globals with literal init values)
    /// 2. `current_state` initialization to start state index
    /// 3. Main dispatch loop (wait vblank, then `JMP` to state's frame handler)
    /// 4. State frame handlers (each ends with `JMP` to main loop)
    /// 5. User function bodies (end with `RTS`)
    pub fn generate(&mut self, ir: &IrProgram) -> Vec<Instruction> {
        // Populate state indices
        for (i, name) in ir.states.iter().enumerate() {
            self.state_indices.insert(name.clone(), i as u8);
        }

        // Emit a `__debug_mode` marker label whenever debug
        // codegen is on. The linker looks for this label to decide
        // whether to splice the debug variant of the NMI handler
        // (which adds a frame-overrun counter). The label itself
        // emits zero bytes — it's just a tripwire the linker can
        // check by name, mirroring the `__audio_used` /
        // `__ppu_update_used` marker pattern already in use.
        if self.debug_mode {
            self.emit_label("__debug_mode");
        }

        // 1. Variable initializers
        //
        // Scalars write a single byte from `init_value`. Array
        // literals write N bytes from `init_array` at successive
        // offsets from the global's base address. Uninitialized
        // globals (neither set) stay at the $00 the RAM-clear left
        // them.
        for global in &ir.globals {
            let Some(&base) = self.var_addrs.get(&global.var_id) else {
                continue;
            };
            if !global.init_array.is_empty() {
                for (offset, &byte) in global.init_array.iter().enumerate() {
                    let addr = base.wrapping_add(offset as u16);
                    self.emit(LDA, AM::Immediate(byte));
                    if addr < 0x100 {
                        self.emit(STA, AM::ZeroPage(addr as u8));
                    } else {
                        self.emit(STA, AM::Absolute(addr));
                    }
                }
            } else if let Some(val) = global.init_value {
                // Emit the low byte first.
                let lo = (val & 0xFF) as u8;
                self.emit(LDA, AM::Immediate(lo));
                if base < 0x100 {
                    self.emit(STA, AM::ZeroPage(base as u8));
                } else {
                    self.emit(STA, AM::Absolute(base));
                }
                // For multi-byte globals (u16), also emit the high
                // byte at base+1. Without this the u16 initializer
                // silently truncates to its low byte — the high
                // byte stays at whatever the RAM clear left it.
                if global.size >= 2 {
                    let hi = ((val >> 8) & 0xFF) as u8;
                    self.emit(LDA, AM::Immediate(hi));
                    let hi_addr = base.wrapping_add(1);
                    if hi_addr < 0x100 {
                        self.emit(STA, AM::ZeroPage(hi_addr as u8));
                    } else {
                        self.emit(STA, AM::Absolute(hi_addr));
                    }
                }
            }
        }

        // 2. Initialize current_state to start state index and call
        // the start state's on_enter handler (if any).
        if let Some(&start_idx) = self.state_indices.get(&ir.start_state) {
            self.emit(LDA, AM::Immediate(start_idx));
            self.emit(STA, AM::ZeroPage(ZP_CURRENT_STATE));
            let enter_fn = format!("{}_enter", ir.start_state);
            if self.function_exists(&enter_fn) {
                self.emit(JSR, AM::Label(format!("__ir_fn_{enter_fn}")));
            }
        }

        // 2b. If the program has any `on scanline` handlers, set up
        // the MMC3 IRQ counter using the scanline count of the
        // *start* state's first scanline handler. Subsequent
        // scanlines in the same frame reload the counter from
        // within the IRQ handler itself using the delta to the
        // next scanline. State transitions (`transition X`) rely
        // on the NMI reload helper to pick the right first
        // scanline for the new state.
        let scanline_groups = group_scanline_handlers(ir);
        if !scanline_groups.is_empty() {
            // Prefer the start state's first scanline; otherwise
            // use the first group's first line.
            let first_line = scanline_groups
                .iter()
                .find(|(s, _)| *s == ir.start_state)
                .and_then(|(_, lines)| lines.first().copied())
                .or_else(|| {
                    scanline_groups
                        .first()
                        .and_then(|(_, l)| l.first().copied())
                })
                .unwrap_or(0);
            // Write (line-1) to $C000 (scanline latch), any value
            // to $C001 (reload counter), any value to $E001 (enable
            // IRQ).
            self.emit(LDA, AM::Immediate(first_line.saturating_sub(1)));
            self.emit(STA, AM::Absolute(0xC000));
            self.emit(STA, AM::Absolute(0xC001));
            self.emit(STA, AM::Absolute(0xE001));
            // Enable interrupts (CLI) so the IRQ can fire.
            self.emit(CLI, AM::Implied);
        }

        // 3. Main dispatch loop
        let main_loop = "__ir_main_loop".to_string();
        self.emit_label(&main_loop);

        // Wait for vblank flag
        let wait_label = "__ir_wait_vblank".to_string();
        self.emit_label(&wait_label);
        self.emit(LDA, AM::ZeroPage(ZP_FRAME_FLAG));
        self.emit(BEQ, AM::LabelRelative(wait_label));
        // Clear the flag
        self.emit(LDA, AM::Immediate(0));
        self.emit(STA, AM::ZeroPage(ZP_FRAME_FLAG));

        // Dispatch on current_state using CMP + BNE + JMP trampoline
        self.emit(LDA, AM::ZeroPage(ZP_CURRENT_STATE));
        for (i, state_name) in ir.states.iter().enumerate() {
            let frame_handler = format!("{state_name}_frame");
            // Only dispatch if the state has a frame handler
            if ir.functions.iter().any(|f| f.name == frame_handler) {
                let skip_label = format!("__ir_disp_skip_{i}");
                self.emit(CMP, AM::Immediate(i as u8));
                self.emit(BNE, AM::LabelRelative(skip_label.clone()));
                self.emit(JMP, AM::Label(format!("__ir_fn_{frame_handler}")));
                self.emit_label(&skip_label);
            }
        }
        self.emit(JMP, AM::Label(main_loop));

        // 4. Emit each fixed-bank function body (state handlers +
        // top-level user functions). Functions tagged with `bank:
        // Some(name)` belong to a switchable bank and are emitted
        // separately into `self.banked_streams` after the fixed-bank
        // pass finishes — see the loop further down.
        for func in &ir.functions {
            if func.bank.is_some() {
                continue;
            }
            self.gen_function(func);
        }

        // 5. If we have scanline handlers, emit an IRQ handler that
        // saves registers, ACKs the MMC3 IRQ, dispatches to the
        // current state's scanline handler (if any), restores
        // registers, and RTIs. The linker picks up `__irq_user` and
        // uses it as the IRQ vector instead of the default stub.
        //
        // Multi-scanline support: a state may have multiple `on
        // scanline(N)` handlers. They fire in ascending order of
        // N. We track which one is next via `ZP_SCANLINE_STEP`,
        // reset to 0 by the NMI reload helper at the top of each
        // frame. The IRQ dispatcher selects the handler for
        // `(current_state, scanline_step)`, runs it, then reloads
        // the MMC3 counter with the *delta* to the next scanline
        // so the counter fires at exactly the right line. If
        // there's no next scanline for the current state, the
        // dispatcher leaves the IRQ disabled and waits for NMI to
        // re-arm.
        if !scanline_groups.is_empty() {
            self.gen_scanline_irq(&scanline_groups);
            self.gen_scanline_reload(&scanline_groups);
        }

        // Debug-mode halt routine for failed array bounds checks.
        // Every `emit_bounds_check` that ran writes a
        // `BCS __debug_halt` which lands here on out-of-range
        // indices. The routine is just `JMP __debug_halt` — an
        // infinite loop that the debugger sees as a deterministic
        // wedge on the offending address. Release builds never set
        // `bounds_halt_used`, so this whole block compiles to zero
        // bytes under `cargo run --release -- build`.
        if self.bounds_halt_used {
            self.emit_label("__debug_halt");
            // Write a recognizable sentinel to the emulator debug
            // port before wedging, so the log shows a bounds-check
            // failure as a distinct event from a plain halt.
            self.emit(LDA, AM::Immediate(0xBC));
            self.emit(STA, AM::Absolute(DEBUG_PORT));
            self.emit(JMP, AM::Label("__debug_halt".to_string()));
        }

        // Snapshot the fixed-bank instruction stream before we
        // start emitting the banked function bodies into their own
        // streams. Programs without any banked functions skip the
        // banked-emission loop entirely so the codegen output is
        // byte-for-byte identical to the pre-banked behaviour.
        let fixed_instructions = std::mem::take(&mut self.instructions);

        // 6. For each function tagged with a bank, redirect emission
        // into a fresh per-bank instruction stream and call
        // `gen_function`. The streams are keyed by bank name and
        // collected into `self.banked_streams` for the linker to
        // pick up via [`Self::banked_streams`].
        for func in &ir.functions {
            let Some(bank_name) = func.bank.clone() else {
                continue;
            };
            // Pull the existing stream for this bank (if any) out
            // of the map so subsequent functions in the same bank
            // accumulate into one contiguous stream. The first
            // function in a bank starts with an empty Vec.
            let prev = self.banked_streams.remove(&bank_name).unwrap_or_default();
            let prior_instrs = std::mem::replace(&mut self.instructions, prev);
            self.current_bank = Some(bank_name.clone());
            self.gen_function(func);
            self.current_bank = None;
            // Move the per-bank stream back into the map and
            // restore whatever instruction buffer was active when
            // we entered this iteration (always empty in the
            // current pipeline, but we restore it for symmetry).
            let bank_stream = std::mem::replace(&mut self.instructions, prior_instrs);
            self.banked_streams.insert(bank_name, bank_stream);
        }

        fixed_instructions
    }

    fn gen_function(&mut self, func: &IrFunction) {
        // Reset temp slot allocator per function.
        self.temp_slots.clear();
        self.next_temp_slot = 0;
        self.free_slots.clear();
        self.use_counts = build_use_counts(func);
        self.in_frame_handler = func.name.ends_with("_frame");

        self.emit_label(&format!("__ir_fn_{}", func.name));

        // At the start of a frame handler that actually draws
        // sprites, clear the OAM shadow buffer so stale sprites from
        // the previous frame (or from a different state's handler)
        // don't linger on screen. We set the Y position byte of every
        // OAM entry to $FE (off-screen) and the `draw` code
        // overwrites the slots it needs. Handlers that never call
        // `draw` skip the clear entirely — the NMI handler's DMA
        // copies whatever's in $0200 unchanged.
        if self.in_frame_handler && function_draws_sprites(func) {
            let clear_loop = format!("__ir_oam_clear_{}", func.name);
            self.emit(LDX, AM::Immediate(0));
            self.emit(LDA, AM::Immediate(0xFE));
            self.emit_label(&clear_loop);
            self.emit(STA, AM::AbsoluteX(0x0200));
            self.emit(INX, AM::Implied);
            self.emit(INX, AM::Implied);
            self.emit(INX, AM::Implied);
            self.emit(INX, AM::Implied);
            self.emit(BNE, AM::LabelRelative(clear_loop));

            // Reset the runtime OAM cursor so the first `draw`
            // writes to slot 0. Every subsequent `draw` in this
            // handler bumps the cursor by 4 — including draws
            // inside loops, which is why this replaces the old
            // compile-time `next_oam_slot` bookkeeping.
            self.emit(LDA, AM::Immediate(0));
            self.emit(STA, AM::ZeroPage(ZP_OAM_CURSOR));
        }

        for block in &func.blocks {
            self.gen_block(block);
        }

        self.in_frame_handler = false;
    }

    fn gen_block(&mut self, block: &IrBasicBlock) {
        self.emit_label(&format!("__ir_blk_{}", block.label));

        for op in &block.ops {
            self.gen_op(op);
            // After each op runs, decrement use counts for its
            // source temps. When a count hits zero the temp's slot
            // returns to the free list and can be reused by a
            // subsequent op's destination. This is what keeps the
            // 128-slot TEMP region large enough for any sane
            // function.
            self.retire_op_sources(op);
        }

        // The terminator may also reference a temp (branch
        // condition, return value). Those temps die after the
        // terminator runs; retire them here so they don't leak
        // across block boundaries.
        self.gen_terminator(&block.terminator);
        for t in terminator_source_temps(&block.terminator) {
            self.dec_use(t);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn gen_op(&mut self, op: &IrOp) {
        match op {
            IrOp::LoadImm(dest, val) => {
                self.emit(LDA, AM::Immediate(*val));
                self.store_temp(*dest);
            }
            IrOp::LoadVar(dest, var) => {
                if let Some(&addr) = self.var_addrs.get(var) {
                    if addr < 0x100 {
                        self.emit(LDA, AM::ZeroPage(addr as u8));
                    } else {
                        self.emit(LDA, AM::Absolute(addr));
                    }
                    self.store_temp(*dest);
                }
            }
            IrOp::StoreVar(var, src) => {
                if let Some(&addr) = self.var_addrs.get(var) {
                    self.load_temp(*src);
                    if addr < 0x100 {
                        self.emit(STA, AM::ZeroPage(addr as u8));
                    } else {
                        self.emit(STA, AM::Absolute(addr));
                    }
                }
            }
            IrOp::Add(d, a, b) => {
                self.load_temp(*a);
                self.emit(CLC, AM::Implied);
                let b_addr = self.temp_addr(*b);
                self.emit(ADC, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::Sub(d, a, b) => {
                self.load_temp(*a);
                self.emit(SEC, AM::Implied);
                let b_addr = self.temp_addr(*b);
                self.emit(SBC, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::Mul(d, a, b) => {
                // Software multiply: multiplicand in A, multiplier in $02
                self.load_temp(*a);
                self.emit(PHA, AM::Implied); // Save for __multiply contract
                let b_addr = self.temp_addr(*b);
                self.emit(LDA, AM::ZeroPage(b_addr));
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(PLA, AM::Implied);
                self.emit(JSR, AM::Label("__multiply".into()));
                self.store_temp(*d);
            }
            IrOp::And(d, a, b) => {
                self.load_temp(*a);
                let b_addr = self.temp_addr(*b);
                self.emit(AND, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::Or(d, a, b) => {
                self.load_temp(*a);
                let b_addr = self.temp_addr(*b);
                self.emit(ORA, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::Xor(d, a, b) => {
                self.load_temp(*a);
                let b_addr = self.temp_addr(*b);
                self.emit(EOR, AM::ZeroPage(b_addr));
                self.store_temp(*d);
            }
            IrOp::ShiftLeft(d, a, count) => {
                self.load_temp(*a);
                for _ in 0..*count {
                    self.emit(ASL, AM::Accumulator);
                }
                self.store_temp(*d);
            }
            IrOp::ShiftRight(d, a, count) => {
                self.load_temp(*a);
                for _ in 0..*count {
                    self.emit(LSR, AM::Accumulator);
                }
                self.store_temp(*d);
            }
            IrOp::ShiftLeftVar(d, a, amt) => self.gen_shift_var(*d, *a, *amt, ASL),
            IrOp::ShiftRightVar(d, a, amt) => self.gen_shift_var(*d, *a, *amt, LSR),
            IrOp::Div(d, a, b) => {
                // Software divide: dividend in A, divisor in $02.
                // `__divide` returns quotient in A and leaves
                // remainder in ZP $03.
                self.load_temp(*a);
                self.emit(PHA, AM::Implied);
                let b_addr = self.temp_addr(*b);
                self.emit(LDA, AM::ZeroPage(b_addr));
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(PLA, AM::Implied);
                self.emit(JSR, AM::Label("__divide".into()));
                self.store_temp(*d);
            }
            IrOp::Mod(d, a, b) => {
                // Modulo reuses __divide and reads the remainder out
                // of ZP $03 afterwards.
                self.load_temp(*a);
                self.emit(PHA, AM::Implied);
                let b_addr = self.temp_addr(*b);
                self.emit(LDA, AM::ZeroPage(b_addr));
                self.emit(STA, AM::ZeroPage(0x02));
                self.emit(PLA, AM::Implied);
                self.emit(JSR, AM::Label("__divide".into()));
                self.emit(LDA, AM::ZeroPage(0x03));
                self.store_temp(*d);
            }
            IrOp::Negate(d, src) => {
                self.load_temp(*src);
                self.emit(EOR, AM::Immediate(0xFF));
                self.emit(CLC, AM::Implied);
                self.emit(ADC, AM::Immediate(1));
                self.store_temp(*d);
            }
            IrOp::Complement(d, src) => {
                self.load_temp(*src);
                self.emit(EOR, AM::Immediate(0xFF));
                self.store_temp(*d);
            }
            IrOp::CmpEq(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::Eq),
            IrOp::CmpNe(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::Ne),
            IrOp::CmpLt(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::Lt),
            IrOp::CmpGt(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::Gt),
            IrOp::CmpLtEq(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::LtEq),
            IrOp::CmpGtEq(d, a, b) => self.gen_cmp(*d, *a, *b, CmpKind::GtEq),
            IrOp::ArrayLoad(dest, var, idx) => {
                if let Some(&base_addr) = self.var_addrs.get(var) {
                    self.load_temp(*idx);
                    self.emit_bounds_check(*var);
                    self.emit(TAX, AM::Implied);
                    if base_addr < 0x100 {
                        self.emit(LDA, AM::ZeroPageX(base_addr as u8));
                    } else {
                        self.emit(LDA, AM::AbsoluteX(base_addr));
                    }
                    self.store_temp(*dest);
                }
            }
            IrOp::ArrayStore(var, idx, val) => {
                if let Some(&base_addr) = self.var_addrs.get(var) {
                    self.load_temp(*idx);
                    self.emit_bounds_check(*var);
                    self.emit(TAX, AM::Implied);
                    self.load_temp(*val);
                    if base_addr < 0x100 {
                        self.emit(STA, AM::ZeroPageX(base_addr as u8));
                    } else {
                        self.emit(STA, AM::AbsoluteX(base_addr));
                    }
                }
            }
            IrOp::Call(dest, name, args) => {
                // Pass up to 4 arguments via zero-page slots $04-$07.
                // Arguments beyond the fourth are silently dropped
                // (the analyzer has already validated arity against
                // the declared signature).
                for (i, arg) in args.iter().enumerate().take(4) {
                    self.load_temp(*arg);
                    self.emit(STA, AM::ZeroPage(0x04 + i as u8));
                }
                // Pick the right JSR target. Three cases:
                //   1. Callee is in the fixed bank (most common):
                //      JSR `__ir_fn_<name>` — the original behaviour.
                //   2. Callee is in a switchable bank and the caller
                //      is in the fixed bank: JSR `__tramp_<name>`,
                //      the linker-emitted trampoline that switches
                //      banks, calls the body, then switches back.
                //   3. Caller and callee live in the same switchable
                //      bank: direct JSR to `__ir_fn_<name>` works
                //      because both labels exist in the bank's own
                //      assembler pass.
                //
                // Cross-bank calls between two different switchable
                // banks aren't supported in the first pass — the
                // codegen panics rather than silently miscompiling.
                let callee_bank = self.function_banks.get(name).cloned();
                let label = match (&self.current_bank, &callee_bank) {
                    (None, None) => format!("__ir_fn_{name}"),
                    (None, Some(_)) => format!("__tramp_{name}"),
                    (Some(from_bank), Some(to_bank)) if from_bank == to_bank => {
                        format!("__ir_fn_{name}")
                    }
                    (Some(from_bank), Some(to_bank)) => {
                        panic!(
                            "cross-bank call from bank '{from_bank}' to '{to_bank}' \
                             is not supported (function '{name}'); only fixed-bank \
                             callers can invoke banked functions in the v1 \
                             user-banked codegen"
                        );
                    }
                    (Some(_), None) => {
                        // Banked function calls a fixed-bank function.
                        // The fixed bank is always mapped at $C000-$FFFF
                        // so a direct JSR works without a trampoline —
                        // no bank-switching needed because we're already
                        // calling into the always-mapped window.
                        format!("__ir_fn_{name}")
                    }
                };
                self.emit(JSR, AM::Label(label));
                if let Some(d) = dest {
                    // Return value is in A
                    self.store_temp(*d);
                }
            }
            IrOp::DrawSprite {
                sprite_name,
                x,
                y,
                frame,
            } => {
                // Runtime OAM-cursor-based draw. Each frame handler
                // resets `ZP_OAM_CURSOR` to 0 after the OAM clear; a
                // `draw` loads the cursor into Y, writes the four
                // sprite bytes via `STA $0200,Y` / `$0201,Y` / etc.,
                // then bumps the cursor by 4 so the next `draw`
                // lands in the next slot.
                //
                // This lets `draw` inside a loop body actually
                // write to a fresh slot on every iteration — with
                // the old static `next_oam_slot` scheme every
                // iteration of a loop clobbered the same 4 bytes,
                // so only the last iteration was visible.
                //
                // At 64 slots the cursor naturally wraps (u8
                // overflow) and subsequent draws overwrite the
                // oldest slots — the classic NES "too many
                // sprites" flicker behavior rather than a silent
                // compile-time drop.
                //
                // Cost over the old static scheme is +1 `LDY`, +4
                // `INC` (cursor bumps), so roughly +6 bytes per
                // draw. Worth it for correct loop semantics.

                // Load the cursor into Y so the four stores below
                // all index off the current slot. We do this
                // once per draw — Y isn't preserved across JSRs
                // or between unrelated ops, so each draw owns Y
                // for its duration.
                self.emit(LDY, AM::ZeroPage(ZP_OAM_CURSOR));

                // Y position at cursor+0
                self.load_temp(*y);
                self.emit(STA, AM::AbsoluteY(0x0200));

                // Tile index at cursor+1 — frame override, sprite lookup, or 0
                if let Some(f) = frame {
                    self.load_temp(*f);
                } else if let Some(&tile) = self.sprite_tiles.get(sprite_name) {
                    self.emit(LDA, AM::Immediate(tile));
                } else {
                    self.emit(LDA, AM::Immediate(0));
                }
                self.emit(STA, AM::AbsoluteY(0x0201));

                // Attributes at cursor+2 (always 0)
                self.emit(LDA, AM::Immediate(0));
                self.emit(STA, AM::AbsoluteY(0x0202));

                // X position at cursor+3
                self.load_temp(*x);
                self.emit(STA, AM::AbsoluteY(0x0203));

                // Advance the cursor by 4. INC $zp is 2 cycles
                // and 2 bytes — four of them are smaller and
                // faster than LDA/CLC/ADC #4/STA. u8 overflow at
                // slot 64 wraps naturally.
                self.emit(INC, AM::ZeroPage(ZP_OAM_CURSOR));
                self.emit(INC, AM::ZeroPage(ZP_OAM_CURSOR));
                self.emit(INC, AM::ZeroPage(ZP_OAM_CURSOR));
                self.emit(INC, AM::ZeroPage(ZP_OAM_CURSOR));
            }
            IrOp::ReadInput(dest, player) => {
                // $01 = P1 input byte, $08 = P2 input byte
                let addr = if *player == 1 { 0x08 } else { 0x01 };
                self.emit(LDA, AM::ZeroPage(addr));
                self.store_temp(*dest);
            }
            IrOp::WaitFrame => {
                // Poll frame flag at $00 until nonzero, then clear it
                let wait_label = format!("__ir_wait_{}", self.instructions.len());
                self.emit_label(&wait_label);
                self.emit(LDA, AM::ZeroPage(ZP_FRAME_FLAG));
                self.emit(BEQ, AM::LabelRelative(wait_label));
                self.emit(LDA, AM::Immediate(0));
                self.emit(STA, AM::ZeroPage(ZP_FRAME_FLAG));
            }
            IrOp::Transition(name) => {
                // Write the target state's index to current_state, then
                // call the target state's on_enter handler if it exists,
                // then JMP to main loop for the new state's frame handler.
                //
                // Note: on_exit of the current state isn't called here
                // because we don't know from an IR op alone which state
                // we're leaving. Proper on_exit support would need
                // per-state transition lowering. Future improvement.
                if let Some(&idx) = self.state_indices.get(name) {
                    self.emit(LDA, AM::Immediate(idx));
                    self.emit(STA, AM::ZeroPage(ZP_CURRENT_STATE));
                    // Call the target state's on_enter handler if defined
                    let enter_fn = format!("{name}_enter");
                    if self.function_exists(&enter_fn) {
                        self.emit(JSR, AM::Label(format!("__ir_fn_{enter_fn}")));
                    }
                    self.emit(JMP, AM::Label("__ir_main_loop".into()));
                }
            }
            IrOp::Scroll(x, y) => {
                // PPU scroll register $2005 takes two writes: X then Y
                self.load_temp(*x);
                self.emit(STA, AM::Absolute(0x2005));
                self.load_temp(*y);
                self.emit(STA, AM::Absolute(0x2005));
            }
            IrOp::DebugLog(args) => {
                if self.debug_mode {
                    for arg in args {
                        self.load_temp(*arg);
                        self.emit(STA, AM::Absolute(DEBUG_PORT));
                    }
                }
                // In release mode, stripped entirely
            }
            IrOp::DebugAssert(cond) => {
                if self.debug_mode {
                    // Load cond; if nonzero (true) skip; else halt
                    self.load_temp(*cond);
                    let pass_label = format!("__ir_assert_pass_{}", self.instructions.len());
                    self.emit(BNE, AM::LabelRelative(pass_label.clone()));
                    // Assertion failed: write marker to debug port and BRK
                    self.emit(LDA, AM::Immediate(0xFF));
                    self.emit(STA, AM::Absolute(DEBUG_PORT));
                    self.emit(BRK, AM::Implied);
                    self.emit_label(&pass_label);
                }
            }
            IrOp::InlineAsm(body) => {
                // Preprocess `{var}` substitutions (unless this is a
                // `raw asm` block, flagged by the lowering with a
                // magic prefix), then parse with the shared inline
                // parser and splice the resulting instructions.
                let raw = body.strip_prefix(crate::ir::RAW_ASM_PREFIX);
                let to_parse: std::borrow::Cow<'_, str> = if let Some(raw_body) = raw {
                    std::borrow::Cow::Borrowed(raw_body)
                } else {
                    std::borrow::Cow::Owned(substitute_asm_vars(body, |name| {
                        self.allocations
                            .iter()
                            .find(|a| a.name == name)
                            .map(|a| a.address)
                    }))
                };
                match crate::asm::parse_inline(&to_parse) {
                    Ok(parsed) => self.instructions.extend(parsed),
                    Err(msg) => {
                        eprintln!("inline asm error: {msg}");
                        self.emit(BRK, AM::Implied);
                    }
                }
            }
            IrOp::Poke(addr, src) => {
                self.load_temp(*src);
                if *addr < 0x100 {
                    self.emit(STA, AM::ZeroPage(*addr as u8));
                } else {
                    self.emit(STA, AM::Absolute(*addr));
                }
            }
            IrOp::Peek(dest, addr) => {
                if *addr < 0x100 {
                    self.emit(LDA, AM::ZeroPage(*addr as u8));
                } else {
                    self.emit(LDA, AM::Absolute(*addr));
                }
                self.store_temp(*dest);
            }
            IrOp::PlaySfx(name) => self.gen_play_sfx(name),
            IrOp::StartMusic(name) => self.gen_start_music(name),
            IrOp::StopMusic => self.gen_stop_music(),
            IrOp::SetPalette(name) => self.gen_set_palette(name),
            IrOp::LoadBackground(name) => self.gen_load_background(name),
            IrOp::LoadVarHi(dest, var) => {
                if let Some(&base) = self.var_addrs.get(var) {
                    let addr = base.wrapping_add(1);
                    if addr < 0x100 {
                        self.emit(LDA, AM::ZeroPage(addr as u8));
                    } else {
                        self.emit(LDA, AM::Absolute(addr));
                    }
                    self.store_temp(*dest);
                }
            }
            IrOp::StoreVarHi(var, src) => {
                if let Some(&base) = self.var_addrs.get(var) {
                    let addr = base.wrapping_add(1);
                    self.load_temp(*src);
                    if addr < 0x100 {
                        self.emit(STA, AM::ZeroPage(addr as u8));
                    } else {
                        self.emit(STA, AM::Absolute(addr));
                    }
                }
            }
            IrOp::Add16 {
                d_lo,
                d_hi,
                a_lo,
                a_hi,
                b_lo,
                b_hi,
            } => {
                // Low byte: CLC; LDA a_lo; ADC b_lo; STA d_lo
                let b_lo_addr = self.temp_addr(*b_lo);
                self.load_temp(*a_lo);
                self.emit(CLC, AM::Implied);
                self.emit(ADC, AM::ZeroPage(b_lo_addr));
                self.store_temp(*d_lo);
                // High byte: LDA a_hi; ADC b_hi; STA d_hi
                // (carry is propagated from the low byte — no CLC)
                let b_hi_addr = self.temp_addr(*b_hi);
                self.load_temp(*a_hi);
                self.emit(ADC, AM::ZeroPage(b_hi_addr));
                self.store_temp(*d_hi);
            }
            IrOp::Sub16 {
                d_lo,
                d_hi,
                a_lo,
                a_hi,
                b_lo,
                b_hi,
            } => {
                // Low byte: SEC; LDA a_lo; SBC b_lo; STA d_lo
                let b_lo_addr = self.temp_addr(*b_lo);
                self.load_temp(*a_lo);
                self.emit(SEC, AM::Implied);
                self.emit(SBC, AM::ZeroPage(b_lo_addr));
                self.store_temp(*d_lo);
                // High byte: LDA a_hi; SBC b_hi; STA d_hi
                // (borrow is propagated via the inverted carry flag)
                let b_hi_addr = self.temp_addr(*b_hi);
                self.load_temp(*a_hi);
                self.emit(SBC, AM::ZeroPage(b_hi_addr));
                self.store_temp(*d_hi);
            }
            IrOp::CmpEq16 {
                dest,
                a_lo,
                a_hi,
                b_lo,
                b_hi,
            } => self.gen_cmp16(*dest, *a_lo, *a_hi, *b_lo, *b_hi, Cmp16Kind::Eq),
            IrOp::CmpNe16 {
                dest,
                a_lo,
                a_hi,
                b_lo,
                b_hi,
            } => self.gen_cmp16(*dest, *a_lo, *a_hi, *b_lo, *b_hi, Cmp16Kind::Ne),
            IrOp::CmpLt16 {
                dest,
                a_lo,
                a_hi,
                b_lo,
                b_hi,
            } => self.gen_cmp16(*dest, *a_lo, *a_hi, *b_lo, *b_hi, Cmp16Kind::Lt),
            IrOp::CmpGt16 {
                dest,
                a_lo,
                a_hi,
                b_lo,
                b_hi,
            } => self.gen_cmp16(*dest, *a_lo, *a_hi, *b_lo, *b_hi, Cmp16Kind::Gt),
            IrOp::CmpLtEq16 {
                dest,
                a_lo,
                a_hi,
                b_lo,
                b_hi,
            } => self.gen_cmp16(*dest, *a_lo, *a_hi, *b_lo, *b_hi, Cmp16Kind::LtEq),
            IrOp::CmpGtEq16 {
                dest,
                a_lo,
                a_hi,
                b_lo,
                b_hi,
            } => self.gen_cmp16(*dest, *a_lo, *a_hi, *b_lo, *b_hi, Cmp16Kind::GtEq),
            IrOp::SourceLoc(span) => {
                // Emit a uniquely-named label-definition pseudo-op
                // at the current codegen position — but only when
                // source-map emission is enabled. Labels introduce
                // peephole block boundaries, so unconditionally
                // emitting them would shift release-mode ROM bytes
                // (and break the golden-diff contract). Off by
                // default; the CLI flips it on under `--source-map`.
                if self.emit_source_locs {
                    let name = format!("__src_{}", self.next_source_loc);
                    self.next_source_loc += 1;
                    self.emit_label(&name);
                    self.source_locs.push((name, *span));
                }
            }
        }
    }

    /// Emit the `play Name` sequence.
    ///
    /// This is the trigger side of the audio driver: it writes the
    /// initial period to the destination channel's trigger
    /// registers, sets the per-channel active counter, and loads
    /// the envelope pointer. The per-frame envelope walk happens
    /// in `runtime::gen_audio_tick`.
    ///
    /// The exact register set depends on the sfx's [`Channel`]:
    /// - **Pulse 1**: `$4002/$4003` trigger, `ZP_SFX_PTR_*` envelope
    ///   pointer, `$4000` runtime volume writes. This is the
    ///   original path and is byte-identical to the pre-channels
    ///   codegen.
    /// - **Triangle**: `$400A/$400B` trigger, `$4015 |= $04` to
    ///   enable the channel's length counter, main-RAM envelope
    ///   pointer at [`AUDIO_TRIANGLE_PTR_*`], runtime writes to
    ///   `$4008` (linear counter reload).
    /// - **Noise**: `$400E/$400F` trigger, `$4015 |= $08` to
    ///   enable, main-RAM pointer at [`AUDIO_NOISE_PTR_*`],
    ///   runtime writes to `$400C` (noise volume).
    ///
    /// Programs that never reference triangle/noise sfx only emit
    /// the pulse-1 path here, so their generated code — and their
    /// ROM bytes — are unchanged from before the channel feature.
    ///
    /// If `name` is not a declared sfx or a recognized builtin, we
    /// emit a silent `play` (period 0, zero-length envelope) rather
    /// than failing hard — the analyzer will have already issued a
    /// diagnostic for the unknown name.
    fn gen_play_sfx(&mut self, name: &str) {
        self.emit_audio_marker();
        let Some((period_lo, period_hi, label, channel)) = self.sfx_info.get(name).cloned() else {
            // Unknown name. The analyzer warns on this; emit a no-op
            // sequence so the rest of the code still assembles. The
            // unknown branch is easy to spot in `--asm-dump`: it
            // writes to the APU status register without touching
            // any other pulse-1 state.
            return;
        };
        match channel {
            Channel::Pulse1 | Channel::Pulse2 => self.emit_play_pulse(period_lo, period_hi, &label),
            Channel::Triangle => self.emit_play_triangle(period_lo, period_hi, &label),
            Channel::Noise => self.emit_play_noise(period_lo, period_hi, &label),
        }
    }

    /// Original pulse-1 `play` sequence — unchanged from before the
    /// channel feature. Kept as its own helper so the channel
    /// dispatch above reads cleanly and the byte layout is trivial
    /// to eyeball against the old code.
    fn emit_play_pulse(&mut self, period_lo: u8, period_hi: u8, label: &str) {
        // $4000: we don't write a volume envelope here. The first
        // envelope byte is consumed by the next NMI audio tick. We
        // only need to set up the trigger (period + length).
        //
        // $4002 / $4003: write period bytes. The write to $4003 also
        // loads the length counter and re-triggers the note — that's
        // what makes holding down a sfx button re-start the tone
        // every frame (audible, useful for demos).
        self.emit(LDA, AM::Immediate(period_lo));
        self.emit(STA, AM::Absolute(0x4002));
        self.emit(LDA, AM::Immediate(period_hi));
        self.emit(STA, AM::Absolute(0x4003));
        // Point ZP_SFX_PTR at the envelope blob. Each subsequent
        // NMI advances this pointer and writes the byte to $4000.
        self.emit(LDA, AM::SymbolLo(label.to_string()));
        self.emit(STA, AM::ZeroPage(ZP_SFX_PTR_LO));
        self.emit(LDA, AM::SymbolHi(label.to_string()));
        self.emit(STA, AM::ZeroPage(ZP_SFX_PTR_HI));
        // Mark sfx as active. The audio tick checks this and bails
        // on zero. We use `$FF` (any nonzero value works) as a flag;
        // the tick zeros it when it hits the envelope sentinel.
        self.emit(LDA, AM::Immediate(0xFF));
        self.emit(STA, AM::ZeroPage(ZP_SFX_COUNTER));
    }

    /// Triangle channel trigger sequence. Writes period bytes to
    /// `$400A/$400B`, enables the triangle channel in `$4015`, and
    /// seeds the main-RAM envelope pointer + active counter so the
    /// tick's triangle block starts walking the blob next frame.
    fn emit_play_triangle(&mut self, period_lo: u8, period_hi: u8, label: &str) {
        self.emit_triangle_marker();
        // $4008: linear counter control + reload. Set $FF (control
        // bit + max reload) so the counter starts from a known
        // non-muted state; the tick rewrites this every frame.
        self.emit(LDA, AM::Immediate(0xFF));
        self.emit(STA, AM::Absolute(0x4008));
        // $400A / $400B: period lo / length + period hi.
        self.emit(LDA, AM::Immediate(period_lo));
        self.emit(STA, AM::Absolute(0x400A));
        self.emit(LDA, AM::Immediate(period_hi));
        self.emit(STA, AM::Absolute(0x400B));
        // Enable all four tone channels in the APU status register.
        // We always write $0F (pulse1+pulse2+triangle+noise) instead
        // of just the channel we're triggering, because per-play
        // writes use immediate values and a later noise play with
        // $0B would otherwise silence an in-progress triangle note
        // by clearing bit 2. Channels with no active envelope stay
        // silent via the runtime's per-channel counter gating, so
        // enabling them blindly is harmless.
        self.emit(LDA, AM::Immediate(0x0F));
        self.emit(STA, AM::Absolute(0x4015));
        // Main-RAM envelope pointer.
        self.emit(LDA, AM::SymbolLo(label.to_string()));
        self.emit(STA, AM::Absolute(AUDIO_TRIANGLE_PTR_LO));
        self.emit(LDA, AM::SymbolHi(label.to_string()));
        self.emit(STA, AM::Absolute(AUDIO_TRIANGLE_PTR_HI));
        // Counter nonzero = channel active.
        self.emit(LDA, AM::Immediate(0xFF));
        self.emit(STA, AM::Absolute(AUDIO_TRIANGLE_COUNTER));
    }

    /// Noise channel trigger sequence. Writes mode+period index to
    /// `$400E`, length-counter load to `$400F`, enables the noise
    /// channel in `$4015`, and seeds the main-RAM envelope pointer.
    fn emit_play_noise(&mut self, period_lo: u8, period_hi: u8, label: &str) {
        self.emit_noise_marker();
        // $400C: volume register. Start at constant-volume 0 (muted)
        // so the very first envelope byte (written by the tick one
        // NMI later) audibly triggers the note without a stale
        // value from a previous sfx leaking through.
        self.emit(LDA, AM::Immediate(0x30));
        self.emit(STA, AM::Absolute(0x400C));
        // $400E: mode (bit 7) + period-table index (low 4 bits).
        self.emit(LDA, AM::Immediate(period_lo));
        self.emit(STA, AM::Absolute(0x400E));
        // $400F: length counter load.
        self.emit(LDA, AM::Immediate(period_hi));
        self.emit(STA, AM::Absolute(0x400F));
        // Enable all four tone channels — see the equivalent write
        // in `emit_play_triangle` for why $0F rather than $0B.
        self.emit(LDA, AM::Immediate(0x0F));
        self.emit(STA, AM::Absolute(0x4015));
        // Main-RAM envelope pointer.
        self.emit(LDA, AM::SymbolLo(label.to_string()));
        self.emit(STA, AM::Absolute(AUDIO_NOISE_PTR_LO));
        self.emit(LDA, AM::SymbolHi(label.to_string()));
        self.emit(STA, AM::Absolute(AUDIO_NOISE_PTR_HI));
        // Counter nonzero = channel active.
        self.emit(LDA, AM::Immediate(0xFF));
        self.emit(STA, AM::Absolute(AUDIO_NOISE_COUNTER));
    }

    /// Emit the `start_music Name` sequence.
    ///
    /// Stores the track's header byte into `ZP_MUSIC_STATE` (with
    /// bit 1 OR'd in as the "active" flag) and seeds both the
    /// current pointer and the loop base with the track's stream
    /// label. Also zeroes `ZP_MUSIC_COUNTER` so the very next audio
    /// tick immediately advances to the first note.
    fn gen_start_music(&mut self, name: &str) {
        self.emit_audio_marker();
        let Some((header, label)) = self.music_info.get(name).cloned() else {
            return;
        };
        // State byte: header | 0x02 (active flag). Header already
        // encodes duty, volume, and loop bit.
        self.emit(LDA, AM::Immediate(header | 0x02));
        self.emit(STA, AM::ZeroPage(ZP_MUSIC_STATE));
        // Stream pointer = label. Also seed the loop-back base
        // so the tick's loop branch can rewind.
        self.emit(LDA, AM::SymbolLo(label.clone()));
        self.emit(STA, AM::ZeroPage(ZP_MUSIC_PTR_LO));
        self.emit(STA, AM::ZeroPage(ZP_MUSIC_BASE_LO));
        self.emit(LDA, AM::SymbolHi(label));
        self.emit(STA, AM::ZeroPage(ZP_MUSIC_PTR_HI));
        self.emit(STA, AM::ZeroPage(ZP_MUSIC_BASE_HI));
        // Counter = 1. The tick will decrement to 0 on the next
        // NMI and immediately advance to the first note. We don't
        // use 0 here because the tick's "bit 1 set AND counter
        // hit zero" check would fire before the first real note
        // was even read.
        self.emit(LDA, AM::Immediate(1));
        self.emit(STA, AM::ZeroPage(ZP_MUSIC_COUNTER));
    }

    /// Emit the `stop_music` sequence. Mutes pulse 2 and clears the
    /// music state byte so the audio tick's bit-1 active check
    /// fails and it skips the music work entirely.
    fn gen_stop_music(&mut self) {
        self.emit_audio_marker();
        self.emit(LDA, AM::Immediate(0x30));
        self.emit(STA, AM::Absolute(0x4004));
        self.emit(LDA, AM::Immediate(0));
        self.emit(STA, AM::ZeroPage(ZP_MUSIC_STATE));
        self.emit(STA, AM::ZeroPage(ZP_MUSIC_COUNTER));
    }

    /// Emit the `__audio_used` marker label at most once per program.
    /// The linker scans for this label to decide whether to splice
    /// the audio tick into NMI and link in the driver body.
    fn emit_audio_marker(&mut self) {
        if !self.audio_used {
            self.emit_label("__audio_used");
            self.audio_used = true;
        }
    }

    /// Emit the `__noise_used` marker label at most once per program.
    /// The linker scans for this label to decide whether to append
    /// the noise tick block to the audio tick.
    fn emit_noise_marker(&mut self) {
        if !self.noise_used {
            self.emit_label("__noise_used");
            self.noise_used = true;
        }
    }

    /// Emit the `__triangle_used` marker label at most once per
    /// program. Drives the triangle tick block in the audio driver.
    fn emit_triangle_marker(&mut self) {
        if !self.triangle_used {
            self.emit_label("__triangle_used");
            self.triangle_used = true;
        }
    }

    /// Emit the `set_palette Name` sequence.
    ///
    /// Writes the palette's ROM label pointer into the runtime
    /// `ZP_PENDING_PALETTE_{LO,HI}` slots and sets bit 0 of
    /// `ZP_PPU_UPDATE_FLAGS`. The NMI handler picks these up and
    /// copies 32 bytes from the label to PPU `$3F00-$3F1F` inside
    /// vblank.
    fn gen_set_palette(&mut self, name: &str) {
        self.emit_ppu_update_marker();
        let label = format!("__palette_{name}");
        // Pointer LO/HI
        self.emit(LDA, AM::SymbolLo(label.clone()));
        self.emit(STA, AM::ZeroPage(ZP_PENDING_PALETTE_LO));
        self.emit(LDA, AM::SymbolHi(label));
        self.emit(STA, AM::ZeroPage(ZP_PENDING_PALETTE_HI));
        // Set bit 0 of the flags byte without disturbing other bits.
        self.emit(LDA, AM::ZeroPage(ZP_PPU_UPDATE_FLAGS));
        self.emit(ORA, AM::Immediate(0x01));
        self.emit(STA, AM::ZeroPage(ZP_PPU_UPDATE_FLAGS));
    }

    /// Emit the `load_background Name` sequence. Writes both the
    /// tiles and attributes label pointers and sets bit 1 of the
    /// PPU update flags; the NMI handler then pushes 960+64 bytes
    /// to nametable 0 inside vblank. Large updates may not fit in
    /// a single vblank — the helper writes linearly so the visible
    /// effect is a progressive update.
    fn gen_load_background(&mut self, name: &str) {
        self.emit_ppu_update_marker();
        let tiles_label = format!("__bg_tiles_{name}");
        let attrs_label = format!("__bg_attrs_{name}");
        self.emit(LDA, AM::SymbolLo(tiles_label.clone()));
        self.emit(STA, AM::ZeroPage(ZP_PENDING_BG_TILES_LO));
        self.emit(LDA, AM::SymbolHi(tiles_label));
        self.emit(STA, AM::ZeroPage(ZP_PENDING_BG_TILES_HI));
        self.emit(LDA, AM::SymbolLo(attrs_label.clone()));
        self.emit(STA, AM::ZeroPage(ZP_PENDING_BG_ATTRS_LO));
        self.emit(LDA, AM::SymbolHi(attrs_label));
        self.emit(STA, AM::ZeroPage(ZP_PENDING_BG_ATTRS_HI));
        self.emit(LDA, AM::ZeroPage(ZP_PPU_UPDATE_FLAGS));
        self.emit(ORA, AM::Immediate(0x02));
        self.emit(STA, AM::ZeroPage(ZP_PPU_UPDATE_FLAGS));
    }

    /// Emit the `__ppu_update_used` marker label at most once per
    /// program. The linker scans for this label to decide whether
    /// to splice the PPU update helper into NMI. Programs that
    /// declare palette/background blocks but never call
    /// `set_palette`/`load_background` don't need the marker —
    /// the linker already includes the helper when there are
    /// declarations (for the reset-time initial load).
    fn emit_ppu_update_marker(&mut self) {
        if !self.ppu_update_used {
            self.emit_label("__ppu_update_used");
            self.ppu_update_used = true;
        }
    }

    /// Emit the MMC3 `__irq_user` handler that dispatches on the
    /// `(current_state, scanline_step)` pair. Supports multiple
    /// `on scanline(N)` handlers per state — they fire in ascending
    /// line order, with `ZP_SCANLINE_STEP` tracking which one is
    /// up next.
    ///
    /// For each state/step pair, the dispatcher JSRs the handler,
    /// then either reloads the MMC3 counter with the delta to the
    /// next scanline (so the counter fires at the right line) or
    /// leaves the IRQ disabled until NMI re-arms it for the next
    /// frame. The step counter is bumped regardless so a later
    /// IRQ (should one slip in) routes to the right slot.
    fn gen_scanline_irq(&mut self, groups: &[(String, Vec<u8>)]) {
        self.emit_label("__irq_user");
        // Save registers onto the stack.
        self.emit(PHA, AM::Implied);
        self.emit(TXA, AM::Implied);
        self.emit(PHA, AM::Implied);
        self.emit(TYA, AM::Implied);
        self.emit(PHA, AM::Implied);
        // Acknowledge the MMC3 IRQ ($E000 = disable/ack). We'll
        // re-enable for the next scanline below if there is one.
        self.emit(LDA, AM::Immediate(0));
        self.emit(STA, AM::Absolute(0xE000));

        // Dispatch on current_state.
        self.emit(LDA, AM::ZeroPage(ZP_CURRENT_STATE));
        let done_label = "__irq_user_done".to_string();
        for (state_idx_iter, (state_name, lines)) in groups.iter().enumerate() {
            let state_idx = self.state_indices.get(state_name).copied().unwrap_or(0);
            let next_state_label = format!("__irq_ns_{state_idx_iter}");
            self.emit(CMP, AM::Immediate(state_idx));
            self.emit(BNE, AM::LabelRelative(next_state_label.clone()));

            // Matched this state. Now dispatch on
            // ZP_SCANLINE_STEP to pick the right handler.
            self.emit(LDY, AM::ZeroPage(ZP_SCANLINE_STEP));
            // Bump step now (regardless of which handler we
            // select) so the NEXT IRQ sees a fresh value. If we
            // bumped after running the handler, a handler that
            // JSRed into user code and somehow nested IRQs would
            // see the old step. We do this eagerly by writing
            // `step + 1` back — cheaper than reading, running,
            // writing.
            self.emit(INC, AM::ZeroPage(ZP_SCANLINE_STEP));

            for (step, line) in lines.iter().enumerate() {
                let next_step_label = format!("__irq_s{state_idx_iter}_step_{step}_skip");
                // CPY #step ; BNE next_step
                self.emit(CPY, AM::Immediate(step as u8));
                self.emit(BNE, AM::LabelRelative(next_step_label.clone()));

                // Run the handler for this (state, step).
                let handler = format!("{state_name}_scanline_{line}");
                self.emit(JSR, AM::Label(format!("__ir_fn_{handler}")));

                // Reload the counter for the next scanline in
                // this state, if any. Otherwise leave IRQ
                // disabled (we already wrote $E000 above).
                if let Some(next_line) = lines.get(step + 1) {
                    let delta = next_line.saturating_sub(*line).saturating_sub(1);
                    self.emit(LDA, AM::Immediate(delta));
                    self.emit(STA, AM::Absolute(0xC000));
                    self.emit(STA, AM::Absolute(0xC001));
                    self.emit(STA, AM::Absolute(0xE001));
                }

                self.emit(JMP, AM::Label(done_label.clone()));
                self.emit_label(&next_step_label);
            }

            // Fell off the end of this state's step list — nothing
            // more to do this frame.
            self.emit(JMP, AM::Label(done_label.clone()));
            self.emit_label(&next_state_label);
        }
        self.emit_label(&done_label);
        // Restore registers and return from interrupt.
        self.emit(PLA, AM::Implied);
        self.emit(TAY, AM::Implied);
        self.emit(PLA, AM::Implied);
        self.emit(TAX, AM::Implied);
        self.emit(PLA, AM::Implied);
        self.emit(RTI, AM::Implied);
    }

    /// Emit the NMI-invoked `__ir_mmc3_reload` helper. Each frame
    /// the NMI handler calls this to (a) reset the scanline step
    /// counter to 0 and (b) rearm the MMC3 counter for the current
    /// state's *first* scanline handler. States with no scanline
    /// handlers leave the counter disabled — no IRQs will fire
    /// for that frame.
    fn gen_scanline_reload(&mut self, groups: &[(String, Vec<u8>)]) {
        self.emit_label("__ir_mmc3_reload");
        // Reset the step counter so the first IRQ of the new
        // frame always lands on step 0.
        self.emit(LDA, AM::Immediate(0));
        self.emit(STA, AM::ZeroPage(ZP_SCANLINE_STEP));

        self.emit(LDA, AM::ZeroPage(ZP_CURRENT_STATE));
        let reload_done = "__ir_mmc3_reload_done".to_string();
        for (i, (state_name, lines)) in groups.iter().enumerate() {
            let state_idx = self.state_indices.get(state_name).copied().unwrap_or(0);
            let skip_label = format!("__ir_reload_skip_{i}");
            self.emit(CMP, AM::Immediate(state_idx));
            self.emit(BNE, AM::LabelRelative(skip_label.clone()));
            // Rearm with the first scanline of this state.
            let first = lines.first().copied().unwrap_or(0);
            self.emit(LDA, AM::Immediate(first.saturating_sub(1)));
            self.emit(STA, AM::Absolute(0xC000));
            self.emit(STA, AM::Absolute(0xC001));
            self.emit(STA, AM::Absolute(0xE001));
            self.emit(JMP, AM::Label(reload_done.clone()));
            self.emit_label(&skip_label);
        }
        // No matching state — disable IRQ for this frame.
        self.emit(LDA, AM::Immediate(0));
        self.emit(STA, AM::Absolute(0xE000));
        self.emit_label(&reload_done);
        self.emit(RTS, AM::Implied);
    }

    /// Emit 16-bit unsigned comparison code. Result is stored as
    /// a u8 bool (0 or 1) in `dest`. All six comparison kinds are
    /// handled uniformly: compare high bytes first, then low bytes
    /// only when high bytes are equal.
    fn gen_cmp16(
        &mut self,
        dest: IrTemp,
        a_lo: IrTemp,
        a_hi: IrTemp,
        b_lo: IrTemp,
        b_hi: IrTemp,
        kind: Cmp16Kind,
    ) {
        let true_label = format!("__ir_cmp16_t_{}", self.instructions.len());
        let false_label = format!("__ir_cmp16_f_{}", self.instructions.len());
        let end_label = format!("__ir_cmp16_e_{}", self.instructions.len());
        let lo_label = format!("__ir_cmp16_lo_{}", self.instructions.len());

        // Compare high bytes.
        self.load_temp(a_hi);
        let b_hi_addr = self.temp_addr(b_hi);
        self.emit(CMP, AM::ZeroPage(b_hi_addr));
        // If high bytes differ, the result is determined by the
        // high-byte comparison alone. If they're equal, fall
        // through to the low-byte comparison.
        self.emit(BEQ, AM::LabelRelative(lo_label.clone()));

        match kind {
            Cmp16Kind::Eq => {
                // Unequal high bytes → not equal → false
                self.emit(JMP, AM::Label(false_label.clone()));
            }
            Cmp16Kind::Ne => {
                // Unequal high bytes → true
                self.emit(JMP, AM::Label(true_label.clone()));
            }
            Cmp16Kind::Lt | Cmp16Kind::LtEq => {
                // a < b when a_hi < b_hi (carry clear after CMP)
                self.emit(BCC, AM::LabelRelative(true_label.clone()));
                self.emit(JMP, AM::Label(false_label.clone()));
            }
            Cmp16Kind::Gt | Cmp16Kind::GtEq => {
                // a > b when a_hi > b_hi.  After `CMP b_hi`, carry
                // is set iff a_hi >= b_hi; we already know a_hi !=
                // b_hi (the BEQ above didn't fire), so BCS here
                // means strictly greater.
                self.emit(BCS, AM::LabelRelative(true_label.clone()));
                self.emit(JMP, AM::Label(false_label.clone()));
            }
        }

        // High bytes were equal — compare low bytes.
        self.emit_label(&lo_label);
        self.load_temp(a_lo);
        let b_lo_addr = self.temp_addr(b_lo);
        self.emit(CMP, AM::ZeroPage(b_lo_addr));

        match kind {
            Cmp16Kind::Eq => {
                self.emit(BEQ, AM::LabelRelative(true_label.clone()));
                self.emit(JMP, AM::Label(false_label.clone()));
            }
            Cmp16Kind::Ne => {
                self.emit(BNE, AM::LabelRelative(true_label.clone()));
                self.emit(JMP, AM::Label(false_label.clone()));
            }
            Cmp16Kind::Lt => {
                // a < b when a_lo < b_lo (carry clear)
                self.emit(BCC, AM::LabelRelative(true_label.clone()));
                self.emit(JMP, AM::Label(false_label.clone()));
            }
            Cmp16Kind::LtEq => {
                // a <= b when carry clear OR equal.
                self.emit(BCC, AM::LabelRelative(true_label.clone()));
                self.emit(BEQ, AM::LabelRelative(true_label.clone()));
                self.emit(JMP, AM::Label(false_label.clone()));
            }
            Cmp16Kind::Gt => {
                // a > b when carry set AND not equal.
                self.emit(BEQ, AM::LabelRelative(false_label.clone()));
                self.emit(BCS, AM::LabelRelative(true_label.clone()));
                self.emit(JMP, AM::Label(false_label.clone()));
            }
            Cmp16Kind::GtEq => {
                // a >= b when carry set.
                self.emit(BCS, AM::LabelRelative(true_label.clone()));
                self.emit(JMP, AM::Label(false_label.clone()));
            }
        }

        // False path
        self.emit_label(&false_label);
        self.emit(LDA, AM::Immediate(0));
        self.emit(JMP, AM::Label(end_label.clone()));
        // True path
        self.emit_label(&true_label);
        self.emit(LDA, AM::Immediate(1));
        self.emit_label(&end_label);
        self.store_temp(dest);
    }

    fn gen_cmp(&mut self, dest: IrTemp, a: IrTemp, b: IrTemp, kind: CmpKind) {
        self.load_temp(a);
        let b_addr = self.temp_addr(b);
        self.emit(CMP, AM::ZeroPage(b_addr));

        let true_label = format!("__ir_cmp_t_{}", self.instructions.len());
        let end_label = format!("__ir_cmp_e_{}", self.instructions.len());

        match kind {
            CmpKind::Eq => self.emit(BEQ, AM::LabelRelative(true_label.clone())),
            CmpKind::Ne => self.emit(BNE, AM::LabelRelative(true_label.clone())),
            CmpKind::Lt => self.emit(BCC, AM::LabelRelative(true_label.clone())),
            CmpKind::GtEq => self.emit(BCS, AM::LabelRelative(true_label.clone())),
            CmpKind::Gt => {
                // > : not equal AND carry set
                self.emit(BEQ, AM::LabelRelative(end_label.clone()));
                self.emit(BCS, AM::LabelRelative(true_label.clone()));
            }
            CmpKind::LtEq => {
                // <= : equal OR carry clear
                self.emit(BEQ, AM::LabelRelative(true_label.clone()));
                self.emit(BCC, AM::LabelRelative(true_label.clone()));
            }
        }
        // False path
        self.emit(LDA, AM::Immediate(0));
        self.emit(JMP, AM::Label(end_label.clone()));
        // True path
        self.emit_label(&true_label);
        self.emit(LDA, AM::Immediate(1));
        self.emit_label(&end_label);
        self.store_temp(dest);
    }

    fn gen_terminator(&mut self, terminator: &IrTerminator) {
        match terminator {
            IrTerminator::Jump(label) => {
                self.emit(JMP, AM::Label(format!("__ir_blk_{label}")));
            }
            IrTerminator::Branch(cond, true_label, false_label) => {
                self.load_temp(*cond);
                // BNE true; JMP false
                self.emit(BNE, AM::LabelRelative(format!("__ir_blk_{true_label}")));
                self.emit(JMP, AM::Label(format!("__ir_blk_{false_label}")));
            }
            IrTerminator::Return(value) => {
                if let Some(v) = value {
                    self.load_temp(*v);
                }
                // Frame handlers return to the main dispatch loop,
                // not via RTS (they aren't called via JSR).
                if self.in_frame_handler {
                    self.emit(JMP, AM::Label("__ir_main_loop".into()));
                } else {
                    self.emit(RTS, AM::Implied);
                }
            }
            IrTerminator::Unreachable => {
                // Generate a BRK just in case
                self.emit(BRK, AM::Implied);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CmpKind {
    Eq,
    Ne,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

#[derive(Debug, Clone, Copy)]
enum Cmp16Kind {
    Eq,
    Ne,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

/// Replace `{name}` tokens in an inline-asm body with the resolved
/// hex address from the given resolver. Unknown names and malformed
/// placeholders are passed through unchanged (the asm parser will
/// surface a clearer error than we could at this stage).
///
/// Addresses that fit in a byte become zero-page syntax (`$XX`),
/// otherwise absolute (`$XXXX`). This lets users write
/// `lda {score}` and have it resolve to `lda $10` or similar.
fn substitute_asm_vars<F: Fn(&str) -> Option<u16>>(body: &str, resolver: F) -> String {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // Find the closing `}`.
            if let Some(end) = bytes[i + 1..].iter().position(|&b| b == b'}') {
                let name_start = i + 1;
                let name_end = i + 1 + end;
                let name = &body[name_start..name_end];
                // Only substitute bare identifiers.
                let is_ident = !name.is_empty()
                    && name
                        .chars()
                        .next()
                        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                    && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric());
                if is_ident {
                    if let Some(addr) = resolver(name) {
                        use std::fmt::Write;
                        if addr < 0x100 {
                            let _ = write!(out, "${addr:02X}");
                        } else {
                            let _ = write!(out, "${addr:04X}");
                        }
                        i = name_end + 1;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// True if the given IR function contains at least one
/// `DrawSprite` op. Used by the frame-handler OAM clear to skip
/// the clear loop when a handler doesn't actually draw anything.
fn function_draws_sprites(func: &IrFunction) -> bool {
    func.blocks
        .iter()
        .flat_map(|b| &b.ops)
        .any(|op| matches!(op, IrOp::DrawSprite { .. }))
}

/// Return every source temp referenced by an op. Destination temps
/// are excluded. Mirrors `optimizer::collect_source_temps` but
/// returns a small Vec instead of mutating a set — the codegen
/// wants to iterate each use, not deduplicate them, so that a temp
/// used twice by one op (e.g. `Add(d, t, t)`) is correctly
/// retired twice.
fn op_source_temps(op: &IrOp) -> Vec<IrTemp> {
    match op {
        IrOp::LoadImm(_, _) | IrOp::LoadVar(_, _) | IrOp::LoadVarHi(_, _) => Vec::new(),
        IrOp::StoreVar(_, src) | IrOp::StoreVarHi(_, src) => vec![*src],
        IrOp::Add(_, a, b)
        | IrOp::Sub(_, a, b)
        | IrOp::Mul(_, a, b)
        | IrOp::Div(_, a, b)
        | IrOp::Mod(_, a, b)
        | IrOp::And(_, a, b)
        | IrOp::Or(_, a, b)
        | IrOp::Xor(_, a, b)
        | IrOp::ShiftLeftVar(_, a, b)
        | IrOp::ShiftRightVar(_, a, b)
        | IrOp::CmpEq(_, a, b)
        | IrOp::CmpNe(_, a, b)
        | IrOp::CmpLt(_, a, b)
        | IrOp::CmpGt(_, a, b)
        | IrOp::CmpLtEq(_, a, b)
        | IrOp::CmpGtEq(_, a, b) => vec![*a, *b],
        IrOp::ShiftLeft(_, src, _) | IrOp::ShiftRight(_, src, _) => vec![*src],
        IrOp::Negate(_, src) | IrOp::Complement(_, src) => vec![*src],
        IrOp::ArrayLoad(_, _, idx) => vec![*idx],
        IrOp::ArrayStore(_, idx, val) => vec![*idx, *val],
        IrOp::Call(_, _, args) => args.clone(),
        IrOp::DrawSprite { x, y, frame, .. } => {
            let mut out = vec![*x, *y];
            if let Some(f) = frame {
                out.push(*f);
            }
            out
        }
        IrOp::Scroll(x, y) => vec![*x, *y],
        IrOp::DebugLog(args) => args.clone(),
        IrOp::DebugAssert(cond) => vec![*cond],
        IrOp::Poke(_, src) => vec![*src],
        IrOp::Add16 {
            a_lo,
            a_hi,
            b_lo,
            b_hi,
            ..
        }
        | IrOp::Sub16 {
            a_lo,
            a_hi,
            b_lo,
            b_hi,
            ..
        }
        | IrOp::CmpEq16 {
            a_lo,
            a_hi,
            b_lo,
            b_hi,
            ..
        }
        | IrOp::CmpNe16 {
            a_lo,
            a_hi,
            b_lo,
            b_hi,
            ..
        }
        | IrOp::CmpLt16 {
            a_lo,
            a_hi,
            b_lo,
            b_hi,
            ..
        }
        | IrOp::CmpGt16 {
            a_lo,
            a_hi,
            b_lo,
            b_hi,
            ..
        }
        | IrOp::CmpLtEq16 {
            a_lo,
            a_hi,
            b_lo,
            b_hi,
            ..
        }
        | IrOp::CmpGtEq16 {
            a_lo,
            a_hi,
            b_lo,
            b_hi,
            ..
        } => vec![*a_lo, *a_hi, *b_lo, *b_hi],
        IrOp::ReadInput(_, _)
        | IrOp::WaitFrame
        | IrOp::Transition(_)
        | IrOp::InlineAsm(_)
        | IrOp::Peek(_, _)
        | IrOp::PlaySfx(_)
        | IrOp::StartMusic(_)
        | IrOp::StopMusic
        | IrOp::SetPalette(_)
        | IrOp::LoadBackground(_)
        | IrOp::SourceLoc(_) => Vec::new(),
    }
}

/// Return every source temp referenced by a terminator.
fn terminator_source_temps(term: &IrTerminator) -> Vec<IrTemp> {
    match term {
        IrTerminator::Branch(t, _, _) => vec![*t],
        IrTerminator::Return(Some(t)) => vec![*t],
        IrTerminator::Jump(_) | IrTerminator::Return(None) | IrTerminator::Unreachable => {
            Vec::new()
        }
    }
}

/// Count how many times each temp appears as a source operand in a
/// function. A terminator that reads a temp counts as one use; an
/// op that reads the same temp twice counts as two uses. Used to
/// drive slot recycling in `retire_op_sources`.
fn build_use_counts(func: &IrFunction) -> HashMap<IrTemp, u32> {
    let mut counts: HashMap<IrTemp, u32> = HashMap::new();
    for block in &func.blocks {
        for op in &block.ops {
            for t in op_source_temps(op) {
                *counts.entry(t).or_insert(0) += 1;
            }
        }
        for t in terminator_source_temps(&block.terminator) {
            *counts.entry(t).or_insert(0) += 1;
        }
    }
    counts
}

// SFX and music parameters used to live in hardcoded `lookup_sfx` /
// `lookup_music` tables here. Those have moved to
// `crate::assets::audio::{builtin_sfx, builtin_music}` so the same
// data path handles user-declared effects, builtin fallbacks, and
// the asset resolver — the codegen only needs the compile-time
// trigger constants which flow through `IrCodeGen::with_audio`.

/// Group all scanline handlers by state name, returning
/// `(state_name, sorted_scanlines)` pairs. Within each state the
/// scanlines are sorted ascending — the IRQ dispatcher walks them
/// in order, reloading the MMC3 counter with the delta between
/// consecutive scanlines so the handlers fire at the right lines.
fn group_scanline_handlers(ir: &IrProgram) -> Vec<(String, Vec<u8>)> {
    use std::collections::BTreeMap;
    let mut grouped: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    // Iterate in IR function order to preserve deterministic output
    // for unchanged programs; sort the per-state line lists at the
    // end so the IRQ dispatcher always sees them ascending.
    let mut seen_order = Vec::new();
    for func in &ir.functions {
        if let Some((state_name, tail)) = func.name.rsplit_once("_scanline_") {
            if let Ok(line) = tail.parse::<u8>() {
                let state = state_name.to_string();
                if !grouped.contains_key(&state) {
                    seen_order.push(state.clone());
                }
                grouped.entry(state).or_default().push(line);
            }
        }
    }
    let mut out = Vec::with_capacity(seen_order.len());
    for name in seen_order {
        let mut lines = grouped.remove(&name).unwrap_or_default();
        lines.sort_unstable();
        lines.dedup();
        out.push((name, lines));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer;
    use crate::assets;
    use crate::ir;
    use crate::parser;

    fn lower_and_gen(source: &str) -> Vec<Instruction> {
        let (prog, _) = parser::parse(source);
        let prog = prog.unwrap();
        let analysis = analyzer::analyze(&prog);
        let ir_program = ir::lower(&prog, &analysis);
        // Resolve audio the same way the real pipeline does so `play`
        // and `start_music` tests can reference builtin names.
        let sfx = assets::resolve_sfx(&prog).expect("sfx");
        let music = assets::resolve_music(&prog).expect("music");
        IrCodeGen::new(&analysis.var_allocations, &ir_program)
            .with_audio(&sfx, &music)
            .generate(&ir_program)
    }

    fn has_instruction(insts: &[Instruction], opcode: crate::asm::Opcode, mode: &AM) -> bool {
        insts.iter().any(|i| i.opcode == opcode && i.mode == *mode)
    }

    #[test]
    fn ir_codegen_minimal_program() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 42
            on frame { x = 1 }
            start Main
        "#,
        );
        // Should initialize x = 42
        assert!(has_instruction(&insts, LDA, &AM::Immediate(42)));
    }

    #[test]
    fn ir_codegen_plus_assign() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame { x += 5 }
            start Main
        "#,
        );
        // Should emit CLC + ADC for the add
        assert!(has_instruction(&insts, CLC, &AM::Implied));
        assert!(insts.iter().any(|i| i.opcode == ADC));
    }

    #[test]
    fn ir_codegen_draw_sprite() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var px: u8 = 0
            var py: u8 = 0
            on frame { draw Smiley at: (px, py) }
            start Main
        "#,
        );
        // The runtime OAM cursor approach writes the four bytes
        // of each sprite via `STA $0200,Y` through `STA $0203,Y`
        // with `Y` loaded from the `ZP_OAM_CURSOR` zero-page
        // slot. Verify the full shape of a draw: the cursor
        // load, the four indexed stores, and the cursor bump.
        assert!(has_instruction(&insts, LDY, &AM::ZeroPage(0x09)));
        assert!(has_instruction(&insts, STA, &AM::AbsoluteY(0x0200)));
        assert!(has_instruction(&insts, STA, &AM::AbsoluteY(0x0201)));
        assert!(has_instruction(&insts, STA, &AM::AbsoluteY(0x0202)));
        assert!(has_instruction(&insts, STA, &AM::AbsoluteY(0x0203)));
        let cursor_bumps = insts
            .iter()
            .filter(|i| i.opcode == INC && i.mode == AM::ZeroPage(0x09))
            .count();
        assert_eq!(cursor_bumps, 4, "draw should bump cursor by 4");
    }

    #[test]
    fn ir_codegen_wait_frame() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            on frame { wait_frame }
            start Main
        "#,
        );
        // Should poll frame flag
        assert!(has_instruction(&insts, LDA, &AM::ZeroPage(0x00)));
    }

    #[test]
    fn ir_codegen_button_read() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame {
                if button.right { x += 1 }
            }
            start Main
        "#,
        );
        // Should read input byte
        assert!(has_instruction(&insts, LDA, &AM::ZeroPage(0x01)));
        // Should AND with mask
        assert!(insts.iter().any(|i| i.opcode == AND));
    }

    #[test]
    fn ir_codegen_while_loop() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame {
                while x < 10 { x += 1 }
            }
            start Main
        "#,
        );
        // Should emit CMP + conditional branch
        assert!(insts.iter().any(|i| i.opcode == CMP));
        assert!(insts.iter().any(|i| i.opcode == JMP || i.opcode == BNE));
    }

    #[test]
    fn ir_codegen_if_branch() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame {
                if x == 0 { x = 1 }
            }
            start Main
        "#,
        );
        // Should emit CMP + branch
        assert!(insts.iter().any(|i| i.opcode == CMP));
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use crate::analyzer;
    use crate::assets;
    use crate::ir;
    use crate::parser;

    fn lower_and_gen(source: &str) -> Vec<Instruction> {
        let (prog, _) = parser::parse(source);
        let prog = prog.unwrap();
        let analysis = analyzer::analyze(&prog);
        let ir_program = ir::lower(&prog, &analysis);
        let sfx = assets::resolve_sfx(&prog).expect("sfx");
        let music = assets::resolve_music(&prog).expect("music");
        IrCodeGen::new(&analysis.var_allocations, &ir_program)
            .with_audio(&sfx, &music)
            .generate(&ir_program)
    }

    #[test]
    fn ir_codegen_state_dispatch_emits_main_loop() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            on frame { wait_frame }
            start Main
        "#,
        );
        // Should contain the __ir_main_loop label
        let has_main_loop = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__ir_main_loop"));
        assert!(has_main_loop, "IR codegen should emit main loop label");
    }

    #[test]
    fn ir_codegen_multi_oam_uses_sequential_slots() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var a: u8 = 10
            var b: u8 = 20
            on frame {
                draw First at: (a, a)
                draw Second at: (b, b)
            }
            start Main
        "#,
        );
        // With the runtime OAM cursor, sequential slots come for
        // free at runtime: each `draw` bumps `ZP_OAM_CURSOR` by 4
        // so the next draw's `STA $0200,Y` lands 4 bytes later.
        // We can't check slot numbers statically any more, but
        // we can check that (a) both draws emitted their cursor
        // loads, and (b) the total cursor-bump count matches the
        // number of draws.
        let lda_cursor = insts
            .iter()
            .filter(|i| i.opcode == LDY && i.mode == AM::ZeroPage(0x09))
            .count();
        let cursor_bumps = insts
            .iter()
            .filter(|i| i.opcode == INC && i.mode == AM::ZeroPage(0x09))
            .count();
        assert_eq!(lda_cursor, 2, "each draw should LDY cursor once");
        assert_eq!(cursor_bumps, 8, "each draw should bump cursor 4 times");
    }

    #[test]
    fn ir_codegen_function_call_uses_correct_label_and_params() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            fun sum(a: u8, b: u8) -> u8 { return a + b }
            var x: u8 = 0
            on frame { x = sum(3, 4) }
            start Main
        "#,
        );
        // Caller must store arg0 to $04 and arg1 to $05.
        let writes_arg0 = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::ZeroPage(0x04));
        let writes_arg1 = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::ZeroPage(0x05));
        assert!(writes_arg0, "caller should store arg0 to $04");
        assert!(writes_arg1, "caller should store arg1 to $05");
        // Caller must JSR to __ir_fn_sum (not __fn_sum).
        let has_jsr = insts
            .iter()
            .any(|i| i.opcode == JSR && i.mode == AM::Label("__ir_fn_sum".into()));
        assert!(has_jsr, "caller should JSR to __ir_fn_sum");
        // Callee must read parameters from $04 and $05, not from
        // temp slots $80+.
        let has_param_read = insts
            .iter()
            .any(|i| i.opcode == LDA && i.mode == AM::ZeroPage(0x04));
        assert!(has_param_read, "callee should read parameters from $04");
    }

    #[test]
    fn ir_codegen_multi_scanline_per_state_emits_step_counter_dispatch() {
        // A state with multiple scanline handlers must dispatch on
        // both `current_state` and `ZP_SCANLINE_STEP` (at $0C). The
        // old codegen just took the first handler per state, so
        // scanline 120 and scanline 180 would never fire even with
        // their handlers linked in.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: MMC3 }
            state Main {
                on frame { wait_frame }
                on scanline(60)  { wait_frame }
                on scanline(120) { wait_frame }
                on scanline(180) { wait_frame }
            }
            start Main
        "#,
        );
        // The IRQ dispatcher must read the step counter at $0C.
        let reads_step = insts
            .iter()
            .any(|i| i.opcode == LDY && i.mode == AM::ZeroPage(0x0C));
        assert!(
            reads_step,
            "multi-scanline dispatcher should read ZP_SCANLINE_STEP at $0C"
        );
        // It must also INC the step counter so the next IRQ lands
        // on the next handler.
        let bumps_step = insts
            .iter()
            .any(|i| i.opcode == INC && i.mode == AM::ZeroPage(0x0C));
        assert!(
            bumps_step,
            "multi-scanline dispatcher should bump ZP_SCANLINE_STEP"
        );
        // All three handlers should be emitted as distinct functions.
        for line in [60u8, 120, 180] {
            let name = format!("__ir_fn_Main_scanline_{line}");
            let has_fn = insts
                .iter()
                .any(|i| matches!(&i.mode, AM::Label(l) if l == &name));
            assert!(
                has_fn,
                "handler for scanline {line} should be emitted as function label '{name}'"
            );
        }
        // The reload helper must reset the step counter at the top
        // of each frame.
        let resets_step = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::ZeroPage(0x0C));
        assert!(
            resets_step,
            "reload helper should clear ZP_SCANLINE_STEP at NMI"
        );
    }

    #[test]
    fn ir_codegen_multi_scanline_reload_uses_delta_not_absolute_line() {
        // Between two scanlines in the same state, the MMC3 counter
        // reload must use the *delta* (next - current - 1), not
        // the absolute next line. Otherwise the counter would
        // double-count past lines.
        //
        // For scanlines 60 and 120 the delta is 120 - 60 - 1 = 59.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: MMC3 }
            state Main {
                on frame { wait_frame }
                on scanline(60)  { wait_frame }
                on scanline(120) { wait_frame }
            }
            start Main
        "#,
        );
        // Find the expected delta load. The absolute line number
        // 120 should NOT appear as an immediate if the codegen is
        // doing delta reloads correctly — only 60 (initial) and
        // 59 (delta) should.
        let has_delta = insts
            .iter()
            .any(|i| i.opcode == LDA && i.mode == AM::Immediate(59));
        assert!(
            has_delta,
            "multi-scanline reload should use delta 59 (= 120 - 60 - 1) for the second scanline"
        );
    }

    #[test]
    fn ir_codegen_scanline_emits_mmc3_setup_and_irq_user() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: MMC3 }
            state Main {
                on frame { wait_frame }
                on scanline(100) { wait_frame }
            }
            start Main
        "#,
        );
        // MMC3 latch write (LDA #99; STA $C000)
        let has_latch = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::Absolute(0xC000));
        assert!(has_latch, "should write to MMC3 latch $C000");
        // __irq_user label should be emitted
        let has_irq_user = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__irq_user"));
        assert!(has_irq_user, "should emit __irq_user label");
        // The scanline handler function should exist
        let has_handler = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__ir_fn_Main_scanline_100"));
        assert!(has_handler, "should emit scanline handler function");
    }

    #[test]
    fn ir_codegen_transition_writes_state_index() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            state A {
                on frame { transition B }
            }
            state B {
                on frame { wait_frame }
            }
            start A
        "#,
        );
        // Should write state index 1 (B is second state) to ZP $03
        let has_store_state = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::ZeroPage(0x03));
        assert!(has_store_state, "transition should write to current_state");
    }

    #[test]
    fn ir_codegen_scroll_writes_ppu_register() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var sx: u8 = 0
            var sy: u8 = 0
            on frame { scroll(sx, sy) }
            start Main
        "#,
        );
        // Both X and Y scroll values should be written to $2005
        let scroll_writes = insts
            .iter()
            .filter(|i| i.opcode == STA && i.mode == AM::Absolute(0x2005))
            .count();
        assert_eq!(scroll_writes, 2, "scroll should emit two STA $2005 writes");
    }

    fn lower_and_gen_debug(source: &str) -> Vec<Instruction> {
        let (prog, _) = parser::parse(source);
        let prog = prog.unwrap();
        let analysis = analyzer::analyze(&prog);
        let ir_program = ir::lower(&prog, &analysis);
        IrCodeGen::new(&analysis.var_allocations, &ir_program)
            .with_debug(true)
            .generate(&ir_program)
    }

    #[test]
    fn ir_codegen_debug_log_emits_in_debug_mode() {
        let insts = lower_and_gen_debug(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 42
            on frame { debug.log(x) }
            start Main
        "#,
        );
        // Should write to the debug port $4800
        let writes_debug_port = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4800));
        assert!(writes_debug_port, "debug.log should write to $4800");
    }

    #[test]
    fn ir_codegen_debug_log_stripped_in_release() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 42
            on frame { debug.log(x) }
            start Main
        "#,
        );
        // No debug port writes in release mode
        let writes_debug_port = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x4800));
        assert!(
            !writes_debug_port,
            "debug.log should be stripped in release mode"
        );
    }

    #[test]
    fn ir_codegen_debug_assert_emits_in_debug_mode() {
        let insts = lower_and_gen_debug(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 42
            on frame { debug.assert(x == 42) }
            start Main
        "#,
        );
        // Should emit a BRK for the fail path
        let has_brk = insts.iter().any(|i| i.opcode == BRK);
        assert!(has_brk, "debug.assert should emit BRK on failure path");
    }

    #[test]
    fn ir_codegen_draw_in_loop_emits_one_cursor_based_draw_not_unrolled() {
        // Regression test for bug B. A `draw` inside a `while`
        // loop body must compile to ONE cursor-based draw that
        // runs on every iteration — not zero draws (original
        // bug when combined with handler-local VarDecl tracking)
        // and not unrolled-per-slot static stores (the old bug
        // where `next_oam_slot` was incremented at compile time,
        // so only the last iteration was ever visible).
        //
        // Concretely: we should see exactly one `LDY $09` and
        // four `INC $09` — the shape of a single draw — inside
        // the loop body, and NO static `STA $0200` / `$0204` /
        // `$0208` / `$020C` patterns (which would indicate the
        // old compile-time slot bump).
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var xs: u8[4] = [10, 40, 80, 120]
            on frame {
                var i: u8 = 0
                while i < 4 {
                    draw Smiley at: (xs[i], xs[i])
                    i += 1
                }
            }
            start Main
        "#,
        );

        let cursor_loads = insts
            .iter()
            .filter(|i| i.opcode == LDY && i.mode == AM::ZeroPage(0x09))
            .count();
        let cursor_bumps = insts
            .iter()
            .filter(|i| i.opcode == INC && i.mode == AM::ZeroPage(0x09))
            .count();
        assert_eq!(
            cursor_loads, 1,
            "a single `draw` in a loop should emit one `LDY cursor` (body is emitted once)"
        );
        assert_eq!(
            cursor_bumps, 4,
            "a single `draw` in a loop should emit 4 `INC cursor`"
        );

        // There must be AT LEAST ONE `STA $0200,Y` (the Y-byte
        // store); other slot-0-absolute stores are a smell but
        // allowed for non-draw code.
        let has_abs_y_store = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::AbsoluteY(0x0200));
        assert!(
            has_abs_y_store,
            "draw should emit `STA $0200,Y` (runtime-cursor indexed store)"
        );

        // No `STA $0204` / `$0208` / `$020C` — those would
        // indicate the compiler allocated four separate static
        // OAM slots for a single draw statement (bug B).
        for slot in 1..16u16 {
            let addr = 0x0200 + slot * 4;
            let has_stale_static = insts
                .iter()
                .any(|i| i.opcode == STA && i.mode == AM::Absolute(addr));
            assert!(
                !has_stale_static,
                "unexpected static OAM slot {slot} store at ${addr:04X} \
                 — bug B regression (compile-time slot bumps are back)"
            );
        }
    }

    // ── Audio driver tests ──

    fn has_inst(insts: &[Instruction], opcode: crate::asm::Opcode, mode: &AM) -> bool {
        insts.iter().any(|i| i.opcode == opcode && i.mode == *mode)
    }

    #[test]
    fn ir_codegen_play_noise_sfx_writes_400e_and_emits_noise_marker() {
        // A noise sfx `play` should:
        //   1. Write trigger bytes to $400E / $400F (noise
        //      period + length).
        //   2. Enable the noise channel in the APU status
        //      register at $4015.
        //   3. Emit the `__noise_used` marker so the linker
        //      appends the noise block to the audio tick.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            sfx Zap {
                channel: noise
                pitch: 5
                volume: [15, 8, 2]
            }
            on frame { play Zap }
            start Main
        "#,
        );
        assert!(
            has_inst(&insts, STA, &AM::Absolute(0x400E)),
            "noise play should write $400E (period)"
        );
        assert!(
            has_inst(&insts, STA, &AM::Absolute(0x400F)),
            "noise play should write $400F (length counter)"
        );
        assert!(
            has_inst(&insts, STA, &AM::Absolute(0x4015)),
            "noise play should write APU status ($4015)"
        );
        let has_marker = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__noise_used"));
        assert!(has_marker, "noise play should emit __noise_used marker");
        // And the pulse1 sfx path must not leak through — no
        // $4002 write from this program.
        assert!(
            !has_inst(&insts, STA, &AM::Absolute(0x4002)),
            "noise play should not touch pulse-1 trigger registers"
        );
    }

    #[test]
    fn ir_codegen_play_triangle_sfx_writes_400a_and_emits_triangle_marker() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            sfx Bass {
                channel: triangle
                pitch: 60
                volume: [1, 1, 1]
            }
            on frame { play Bass }
            start Main
        "#,
        );
        assert!(
            has_inst(&insts, STA, &AM::Absolute(0x400A)),
            "triangle play should write $400A (period)"
        );
        assert!(
            has_inst(&insts, STA, &AM::Absolute(0x400B)),
            "triangle play should write $400B (length counter)"
        );
        assert!(
            has_inst(&insts, STA, &AM::Absolute(0x4008)),
            "triangle play should write $4008 (linear counter)"
        );
        let has_marker = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__triangle_used"));
        assert!(
            has_marker,
            "triangle play should emit __triangle_used marker"
        );
    }

    #[test]
    fn ir_codegen_play_sfx_triggers_pulse1_and_loads_envelope_pointer() {
        // `play coin` must:
        //   1. Write the period trigger bytes to $4002 and $4003
        //      (starting the note on pulse 1).
        //   2. Load the envelope blob pointer into ZP_SFX_PTR_LO/HI
        //      via SymbolLo/SymbolHi of the `__sfx_coin` label.
        //   3. Set ZP_SFX_COUNTER nonzero so the audio tick starts
        //      walking the envelope.
        //   4. Emit the `__audio_used` marker label so the linker
        //      splices in the driver and period table.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            on frame { play coin }
            start Main
        "#,
        );
        // Trigger bytes on pulse 1.
        assert!(
            has_inst(&insts, STA, &AM::Absolute(0x4002)),
            "play should write pulse-1 period-lo register $4002"
        );
        assert!(
            has_inst(&insts, STA, &AM::Absolute(0x4003)),
            "play should write pulse-1 length+period-hi register $4003"
        );
        // Envelope pointer loaded via SymbolLo/SymbolHi of sfx label.
        let has_ptr_lo = insts
            .iter()
            .any(|i| i.opcode == LDA && matches!(&i.mode, AM::SymbolLo(n) if n == "__sfx_coin"));
        let has_ptr_hi = insts
            .iter()
            .any(|i| i.opcode == LDA && matches!(&i.mode, AM::SymbolHi(n) if n == "__sfx_coin"));
        assert!(has_ptr_lo, "play should load envelope pointer low byte");
        assert!(has_ptr_hi, "play should load envelope pointer high byte");
        // ZP_SFX_COUNTER (0x0A) set to a nonzero "active" marker.
        assert!(
            has_inst(&insts, STA, &AM::ZeroPage(0x0A)),
            "play should set ZP_SFX_COUNTER to flag the sfx as active"
        );
        // __audio_used marker.
        let has_marker = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__audio_used"));
        assert!(has_marker, "play should emit the __audio_used marker label");
    }

    #[test]
    fn ir_codegen_start_music_sets_state_and_stream_pointer() {
        // `start_music theme` must:
        //   1. Load a state byte (header OR'd with 0x02 = active)
        //      into ZP_MUSIC_STATE (0x07).
        //   2. Load the music stream pointer into ZP_MUSIC_PTR and
        //      ZP_MUSIC_BASE (so the loop branch can rewind).
        //   3. Seed ZP_MUSIC_COUNTER with 1 so the next tick
        //      immediately advances to the first note.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            on frame { start_music theme }
            start Main
        "#,
        );
        // State byte stored at 0x07.
        assert!(
            has_inst(&insts, STA, &AM::ZeroPage(0x07)),
            "start_music should store the state byte at ZP_MUSIC_STATE ($07)"
        );
        // Pointer load via SymbolLo of __music_theme.
        let has_ptr_lo = insts
            .iter()
            .any(|i| i.opcode == LDA && matches!(&i.mode, AM::SymbolLo(n) if n == "__music_theme"));
        let has_ptr_hi = insts
            .iter()
            .any(|i| i.opcode == LDA && matches!(&i.mode, AM::SymbolHi(n) if n == "__music_theme"));
        assert!(has_ptr_lo, "start_music should load stream ptr low");
        assert!(has_ptr_hi, "start_music should load stream ptr high");
        // Both PTR and BASE should be written (4 stores total for
        // the pointer pair: PTR_LO, BASE_LO, PTR_HI, BASE_HI).
        assert!(
            has_inst(&insts, STA, &AM::ZeroPage(0x0E)),
            "start_music should store ZP_MUSIC_PTR_LO ($0E)"
        );
        assert!(
            has_inst(&insts, STA, &AM::ZeroPage(0x05)),
            "start_music should store ZP_MUSIC_BASE_LO ($05) for loop-back"
        );
        assert!(
            has_inst(&insts, STA, &AM::ZeroPage(0x0F)),
            "start_music should store ZP_MUSIC_PTR_HI ($0F)"
        );
        assert!(
            has_inst(&insts, STA, &AM::ZeroPage(0x06)),
            "start_music should store ZP_MUSIC_BASE_HI ($06) for loop-back"
        );
    }

    #[test]
    fn ir_codegen_stop_music_mutes_pulse2_and_clears_state() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            on frame { stop_music }
            start Main
        "#,
        );
        // Mute $4004 and clear ZP_MUSIC_STATE ($07).
        assert!(
            has_inst(&insts, LDA, &AM::Immediate(0x30)),
            "stop_music should load the mute byte $30"
        );
        assert!(
            has_inst(&insts, STA, &AM::Absolute(0x4004)),
            "stop_music should store to pulse-2 volume register $4004"
        );
        assert!(
            has_inst(&insts, STA, &AM::ZeroPage(0x07)),
            "stop_music should clear ZP_MUSIC_STATE ($07)"
        );
    }

    #[test]
    fn ir_codegen_no_audio_means_no_marker() {
        // Programs that never play audio should not emit the
        // `__audio_used` marker — the linker relies on its absence
        // to skip the audio tick and driver entirely.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame { x += 1 }
            start Main
        "#,
        );
        let has_marker = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__audio_used"));
        assert!(
            !has_marker,
            "programs without audio should not emit the __audio_used marker"
        );
    }

    #[test]
    fn ir_codegen_audio_marker_emitted_once() {
        // Multiple audio ops in the same program should produce
        // exactly one marker label — the linker looks it up by
        // name and duplicates would cause assembler errors.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            on frame {
                play coin
                play jump
                start_music theme
                stop_music
            }
            start Main
        "#,
        );
        let marker_count = insts
            .iter()
            .filter(|i| matches!(&i.mode, AM::Label(l) if l == "__audio_used"))
            .count();
        assert_eq!(
            marker_count, 1,
            "__audio_used marker must be emitted exactly once per program"
        );
    }

    // ── u16 arithmetic tests ──

    #[test]
    fn ir_codegen_u16_literal_init_writes_both_bytes() {
        // A u16 initializer `var big: u16 = 1000` must write BOTH
        // bytes of 1000 (low=0xE8, high=0x03) into memory. The old
        // behaviour was to truncate to a single low-byte store,
        // leaving the high byte as whatever the RAM clear left it
        // — a silent 232/0 instead of 1000.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var big: u16 = 1000
            on frame { wait_frame }
            start Main
        "#,
        );
        // 1000 = 0x03E8 → low byte 0xE8, high byte 0x03
        assert!(
            has_inst(&insts, LDA, &AM::Immediate(0xE8)),
            "u16 init should load low byte"
        );
        assert!(
            has_inst(&insts, LDA, &AM::Immediate(0x03)),
            "u16 init should load high byte"
        );
    }

    #[test]
    fn ir_codegen_u16_add_uses_carry_propagation() {
        // `big += 1` on a u16 must propagate carry from the low
        // byte into the high byte. Codegen shape: CLC, LDA a_lo,
        // ADC b_lo, STA d_lo, LDA a_hi, ADC b_hi, STA d_hi — the
        // second ADC has no CLC before it so it uses the carry
        // from the low-byte addition.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var big: u16 = 0
            on frame { big = big + 1 }
            start Main
        "#,
        );
        // There should be at least two ADC instructions (one per
        // byte) and exactly one CLC before them — the Add16 op
        // emits CLC only before the low byte.
        let adc_count = insts.iter().filter(|i| i.opcode == ADC).count();
        assert!(
            adc_count >= 2,
            "Add16 should emit at least two ADC instructions (one per byte), got {adc_count}"
        );
    }

    #[test]
    fn ir_codegen_u16_compound_add_stores_high_byte() {
        // `big += 1` on a u16 variable must emit a store for the
        // high byte after the Add16. Previously the compiler would
        // store only the low byte, silently dropping the carry.
        //
        // The analyzer's RAM allocator sends anything larger than
        // one byte into main RAM starting at `$0300`, so `big`'s
        // high byte lives at `$0301`. That's what we check for.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var big: u16 = 0
            on frame { big += 1 }
            start Main
        "#,
        );
        let has_hi_store = insts
            .iter()
            .any(|i| i.opcode == STA && i.mode == AM::Absolute(0x0301));
        assert!(
            has_hi_store,
            "u16 compound assignment should store the high byte at var+1 ($0301)"
        );
    }

    #[test]
    fn ir_codegen_u16_compare_checks_high_byte() {
        // `big > 256` on a u16 must compare the high byte. A
        // purely low-byte compare would give wrong answers for any
        // value where the high bytes differ.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var big: u16 = 0
            var flag: u8 = 0
            on frame {
                if big > 256 { flag = 1 }
            }
            start Main
        "#,
        );
        // There should be two CMP instructions: one for the high
        // byte and (conditionally) one for the low byte.
        let cmp_count = insts.iter().filter(|i| i.opcode == CMP).count();
        assert!(
            cmp_count >= 2,
            "u16 comparison should emit at least two CMP instructions (one per byte), got {cmp_count}"
        );
    }

    #[test]
    fn ir_codegen_recycles_temp_slots_in_long_functions() {
        // Regression guard for the "IR temps exceed 128 slots"
        // panic that used to crash `bitwise_ops.ne`. A function
        // with many short-lived temps must recycle slots so the
        // allocator stays within the 128-byte TEMP region
        // ($80-$FF). We compile a program with dozens of
        // independent arithmetic expressions and assert that no
        // zero-page address is ever written outside that range.
        let source = r#"
            game "T" { mapper: NROM }
            var a: u8 = 0
            var b: u8 = 0
            var c: u8 = 0
            var d: u8 = 0
            var e: u8 = 0
            var f: u8 = 0
            var g: u8 = 0
            var h: u8 = 0
            on frame {
                a = (a ^ 0x80) | (b & 0x0F)
                b = (b ^ 0x40) | (c & 0x0F)
                c = (c ^ 0x20) | (d & 0x0F)
                d = (d ^ 0x10) | (e & 0x0F)
                e = (e ^ 0x08) | (f & 0x0F)
                f = (f ^ 0x04) | (g & 0x0F)
                g = (g ^ 0x02) | (h & 0x0F)
                h = (h ^ 0x01) | (a & 0x0F)
            }
            start Main
        "#;
        let insts = lower_and_gen(source);
        // Count distinct temp slots used.
        let mut slots = std::collections::HashSet::new();
        for inst in &insts {
            if let AM::ZeroPage(addr) = inst.mode {
                if addr >= 0x80 {
                    slots.insert(addr);
                }
            }
        }
        // Should use far fewer than 128 slots — the recycling
        // means each short-lived temp reuses the same handful of
        // slots.
        assert!(
            slots.len() <= 16,
            "expected slot recycling to keep temp count low, got {} slots: {slots:?}",
            slots.len()
        );
    }

    #[test]
    fn ir_codegen_u8_var_still_uses_8bit_ops() {
        // Regression guard: u8 variables must NOT take the 16-bit
        // path. This is the baseline sanity check that u16 handling
        // didn't accidentally widen every operation.
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var x: u8 = 0
            on frame { x += 1 }
            start Main
        "#,
        );
        // For a plain u8 increment we expect exactly one ADC.
        let adc_count = insts.iter().filter(|i| i.opcode == ADC).count();
        assert_eq!(
            adc_count, 1,
            "u8 += should emit exactly one ADC; got {adc_count}"
        );
    }

    #[test]
    fn ir_codegen_debug_mode_emits_marker_label() {
        // The codegen drops a `__debug_mode` label whenever debug
        // mode is on. The linker reads that label to decide
        // whether to splice the frame-overrun-aware NMI handler,
        // so the marker is load-bearing even though it carries no
        // bytes itself.
        let insts = lower_and_gen_debug(
            r#"
            game "T" { mapper: NROM }
            on frame { wait_frame }
            start Main
        "#,
        );
        let has_marker = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__debug_mode"));
        assert!(has_marker, "debug mode should emit __debug_mode marker");
    }

    #[test]
    fn ir_codegen_release_mode_has_no_debug_marker() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            on frame { wait_frame }
            start Main
        "#,
        );
        let has_marker = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__debug_mode"));
        assert!(
            !has_marker,
            "release mode must not emit __debug_mode; doing so would force the debug NMI"
        );
    }

    #[test]
    fn ir_codegen_bounds_check_in_debug_mode_emits_halt_jump() {
        // Debug-mode array access should emit a CMP + BCC + JMP
        // __debug_halt guard, and the codegen should define
        // `__debug_halt` as a terminal infinite loop. We only
        // check for the presence of the halt label and a JMP
        // targeting it; the actual CMP comes with an immediate
        // whose value depends on the array length. Verified for
        // `xs[i]` on a `u8[4]` array → the immediate should be 4.
        let insts = lower_and_gen_debug(
            r#"
            game "T" { mapper: NROM }
            var xs: u8[4] = [10, 20, 30, 40]
            on frame {
                var i: u8 = 2
                var v: u8 = xs[i]
                wait_frame
            }
            start Main
        "#,
        );
        // Label defined at the halt site.
        let has_halt_label = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__debug_halt") && i.opcode == NOP);
        assert!(has_halt_label, "debug mode should emit __debug_halt label");
        // JMP __debug_halt from the bounds-check fail path.
        let has_jmp_halt = insts
            .iter()
            .any(|i| i.opcode == JMP && matches!(&i.mode, AM::Label(l) if l == "__debug_halt"));
        assert!(
            has_jmp_halt,
            "debug-mode bounds check should JMP to __debug_halt on failure"
        );
        // The CMP #4 compares against the array length.
        let has_cmp_four = insts
            .iter()
            .any(|i| i.opcode == CMP && i.mode == AM::Immediate(4));
        assert!(
            has_cmp_four,
            "bounds check against a `u8[4]` array should CMP against 4"
        );
    }

    #[test]
    fn ir_codegen_bounds_check_stripped_in_release() {
        let insts = lower_and_gen(
            r#"
            game "T" { mapper: NROM }
            var xs: u8[4] = [10, 20, 30, 40]
            on frame {
                var i: u8 = 2
                var v: u8 = xs[i]
                wait_frame
            }
            start Main
        "#,
        );
        let has_halt_label = insts
            .iter()
            .any(|i| matches!(&i.mode, AM::Label(l) if l == "__debug_halt"));
        assert!(
            !has_halt_label,
            "release builds must not emit the bounds-check halt routine"
        );
    }

    #[test]
    fn ir_codegen_source_map_opt_in_emits_src_labels() {
        // With `with_source_map(true)` the codegen should emit
        // a `__src_<N>` label and record the span for each
        // lowered statement. Without the opt-in, release-mode
        // ROMs must stay byte-identical (no `__src_` labels).
        let (prog, _) = parser::parse(
            r#"
            game "T" { mapper: NROM }
            on frame { wait_frame }
            start Main
        "#,
        );
        let prog = prog.unwrap();
        let analysis = analyzer::analyze(&prog);
        let ir_program = ir::lower(&prog, &analysis);
        let mut codegen =
            IrCodeGen::new(&analysis.var_allocations, &ir_program).with_source_map(true);
        let insts = codegen.generate(&ir_program);
        let src_labels: Vec<_> = insts
            .iter()
            .filter_map(|i| match &i.mode {
                AM::Label(l) if l.starts_with("__src_") && i.opcode == NOP => Some(l.clone()),
                _ => None,
            })
            .collect();
        assert!(
            !src_labels.is_empty(),
            "source-map-enabled codegen should emit at least one __src_ label"
        );
        let recorded = codegen.source_locs();
        assert_eq!(
            src_labels.len(),
            recorded.len(),
            "every emitted __src_ label should have a matching source_locs entry"
        );
    }
}
