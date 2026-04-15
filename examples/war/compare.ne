// war/compare.ne — card rank/suit extraction and round resolution.
//
// Cards are packed as `(rank << 4) | suit`; the extractors are
// one shift or one mask each.
//
// Parameter names are unique across the entire program — see
// COMPILER_BUGS.md §1b for why same-named params in different
// functions silently corrupt each other through shared VarIds.

inline fun card_rank(crk_c: u8) -> u8 {
    return crk_c >> 4
}

inline fun card_suit(csu_c: u8) -> u8 {
    return csu_c & 0x0F
}

// Compare two cards by rank. Returns:
//     1 if A wins, 2 if B wins, 0 if they tie
fun compare_cards(cmp_a: u8, cmp_b: u8) -> u8 {
    var cmp_ra: u8 = card_rank(cmp_a)
    var cmp_rb: u8 = card_rank(cmp_b)
    if cmp_ra > cmp_rb {
        return 1
    }
    if cmp_rb > cmp_ra {
        return 2
    }
    return 0
}
