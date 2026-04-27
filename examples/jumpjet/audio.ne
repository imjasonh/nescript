// jumpjet/audio.ne — sfx + music declarations.
//
// SFX run on pulse 1; music runs on pulse 2. The driver latches
// pulse-1 period once on trigger so a scalar `pitch:` is the
// natural form. `envelope:` is the friendly alias for `volume:`.

// ── Missile launch — sharp ascending blip on pulse 1.
sfx Launch {
    duty: 2
    pitch: 0x18
    envelope: [13, 11, 8, 5, 2]
}

// ── Bomb drop — short descending swoosh.
sfx Drop {
    duty: 0
    pitch: 0x80
    envelope: [11, 10, 8, 6, 4, 2]
}

// ── Boom — explosion. Reuses the pulse-1 sfx slot; the noise
//    channel is wired via the `channel: noise` form.
sfx Boom {
    channel: noise
    pitch: 0x06
    envelope: [12, 10, 8, 6, 4, 2, 1]
}

// ── Title music — brisk patrol-march loop on pulse 2.
music TitleMusic {
    duty: 2
    volume: 9
    repeat: true
    tempo: 12
    notes: [
        C4, E4, G4, C5,
        G4, E4, C4, G3,
        D4, F4, A4, D5,
        A4, F4, D4, A3
    ]
}
