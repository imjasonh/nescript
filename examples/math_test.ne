// math_test.ne — example NEScript test file.
//
// Test files are named *_test.ne and are compiled with debug mode enabled
// so that debug.assert statements are active. Run with:
//   cargo run -- test examples/
//
// A test passes when the file compiles without errors. The debug.assert
// calls document the expected runtime behaviour and will halt the NES with
// a BRK instruction if an assertion fails during actual execution.

game "MathTest" {
    mapper: NROM
}

const SPEED: u8 = 3

fun add(a: u8, b: u8) -> u8 {
    return a + b
}

fun double(n: u8) -> u8 {
    return n + n
}

var result: u8 = 0

on frame {
    // Verify addition helper
    result = add(10, 5)
    debug.assert(result == 15)

    // Verify doubling helper
    result = double(7)
    debug.assert(result == 14)

    // Verify constant usage
    result = add(SPEED, 2)
    debug.assert(result == 5)

    wait_frame
}

start Main
