// war/background.ne — the static 32×30 nametable loaded at reset.
//
// NEScript loads the *first* declared `background` into nametable 0
// before rendering is enabled, so whatever this file declares is
// visible on frame 0 of the ROM. We use a single felt-table
// background: a solid dark-green field for the play area. All UI
// (title banner, "PRESS A", win banners, etc.) is drawn on top via
// sprites so we never pay the cost of a full mid-frame nametable
// swap.
//
// Tile 0 is the linker's builtin smiley — we don't want that in the
// game, but it's safe to leave in the top-left corner because the
// whole nametable is painted with a blank tile. Use the legend's
// `.` entry to map every visible cell to tile 0 then rely on
// sub-palette 0 (forest/green/mint) making it look like felt.
//
// The `palette_map:` field is omitted on purpose: every attribute
// byte defaults to 0, which selects bg sub-palette 0 for every
// metatile. That's the felt palette.

background Felt {
    legend {
        ".": 75      // TILE_FELT_BG — sparse mint flecks on dk_green
    }
    map: [
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................",
        "................................"
    ]
}
