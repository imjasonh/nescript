-- Mesen2 .dbg validation probe.
--
-- Run via:
--   Mesen --testRunner <rom.nes> probe.lua --timeout=15
--
-- Mesen auto-loads <rom>.dbg from the same directory as <rom.nes>;
-- this script then queries that label table via emu.getLabelAddress.
-- Communication with the CI wrapper is via the process exit code
-- (Mesen's emu.log writes to an internal buffer, not the process
-- stdout, so exit codes are the only reliable cross-process signal).
--
-- Exit codes:
--   0  = all checks passed
--   1  = `nmi` not found      (runtime entry-point missing → linker bug)
--   2  = `nmi` address is 0   (label resolved to nothing → .dbg parse bug)
--   3  = `Main_frame` not found (state-handler label missing → analyzer/linker bug)
--   4  = `Main_frame` address is 0
--   5  = `main_loop` not found (main-loop entry missing → runtime gating bug)
--   6  = `main_loop` address is 0
--   7  = `irq` not found      (IRQ vector label missing → runtime bug)
--   8  = `irq` address is 0
--
-- Any other non-zero exit indicates Mesen crashed before the probe
-- finished — typically the GLOBALIZATION_INVARIANT/libstdc++
-- collision (fixed by setting DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1
-- on Linux) or a missing libsdl2 dependency.

local function check(label, missing_code, zero_code)
  local info = emu.getLabelAddress(label)
  if info == nil then emu.stop(missing_code) end
  if (info.address or 0) == 0 then emu.stop(zero_code) end
end

check("nmi",        1, 2)
check("Main_frame", 3, 4)
check("main_loop",  5, 6)
check("irq",        7, 8)

-- All four labels resolved to non-zero addresses. That covers:
--   * Segment record parsed (CODE seg at $C000)
--   * Sym records parsed (the four labels above are emitted by
--     `linker::render_dbg` for every NEScript ROM)
--   * Mesen's label name normalization matches our
--     `mlb_symbol_name` filter
emu.stop(0)
