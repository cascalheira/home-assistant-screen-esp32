//! Material Design Icons allowed as page icons and light tile icons
//! (docs/protocol.md).
//!
//! The screen can only draw glyphs compiled into the firmware: each of these
//! codepoints also appears in the `glyph-coverage` text in `ui/app.slint`.

/// `("mdi:name", glyph)` in the editor's picker order.
pub const PAGE_ICONS: &[(&str, &str)] = &[
    ("mdi:home", "\u{F02DC}"),
    ("mdi:sofa", "\u{F04B9}"),
    ("mdi:bed", "\u{F02E3}"),
    ("mdi:silverware-fork-knife", "\u{F0A70}"),
    ("mdi:coffee", "\u{F0176}"),
    ("mdi:shower", "\u{F09A0}"),
    ("mdi:bathtub", "\u{F1818}"),
    ("mdi:toilet", "\u{F09AB}"),
    ("mdi:desk", "\u{F1239}"),
    ("mdi:wardrobe-outline", "\u{F0F91}"),
    ("mdi:baby-face-outline", "\u{F0E7D}"),
    ("mdi:toy-brick-outline", "\u{F128D}"),
    ("mdi:washing-machine", "\u{F072A}"),
    ("mdi:stairs", "\u{F04CD}"),
    ("mdi:door", "\u{F081A}"),
    ("mdi:home-floor-0", "\u{F0DD2}"),
    ("mdi:home-floor-1", "\u{F0D80}"),
    ("mdi:home-floor-2", "\u{F0D81}"),
    ("mdi:home-floor-b", "\u{F0D84}"),
    ("mdi:home-roof", "\u{F112B}"),
    ("mdi:balcony", "\u{F1817}"),
    ("mdi:garage", "\u{F06D9}"),
    ("mdi:car", "\u{F010B}"),
    ("mdi:gate", "\u{F0299}"),
    ("mdi:tree", "\u{F0531}"),
    ("mdi:flower", "\u{F024A}"),
    ("mdi:sprout", "\u{F0E66}"),
    ("mdi:pool", "\u{F0606}"),
    ("mdi:paw", "\u{F03E9}"),
    ("mdi:dumbbell", "\u{F01E6}"),
    ("mdi:television", "\u{F0502}"),
    ("mdi:music", "\u{F075A}"),
    ("mdi:gamepad-variant", "\u{F0297}"),
    ("mdi:lightbulb-group", "\u{F1253}"),
    ("mdi:thermostat", "\u{F0393}"),
    ("mdi:air-conditioner", "\u{F001B}"),
    ("mdi:sun-snowflake-variant", "\u{F1A79}"),
    ("mdi:fan", "\u{F0210}"),
    ("mdi:fire", "\u{F0238}"),
    ("mdi:snowflake", "\u{F0717}"),
    ("mdi:water", "\u{F058C}"),
    ("mdi:power-plug", "\u{F06A5}"),
    ("mdi:weather-sunny", "\u{F0599}"),
    ("mdi:robot", "\u{F06A9}"),
    ("mdi:shield-home", "\u{F068A}"),
    ("mdi:cctv", "\u{F07AE}"),
];

/// Glyph for an `mdi:` icon name; "" for no icon or an unknown name.
pub fn page_glyph(name: &str) -> &'static str {
    PAGE_ICONS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, g)| *g)
        .unwrap_or("")
}

/// Icons a light tile can show instead of the bulb, in the editor's picker
/// order. The tile colour shows on/off, as in Home Assistant.
pub const LIGHT_ICONS: &[(&str, &str)] = &[
    ("mdi:ceiling-light", "\u{F0769}"),
    ("mdi:ceiling-light-multiple", "\u{F18DD}"),
    ("mdi:chandelier", "\u{F1793}"),
    ("mdi:light-recessed", "\u{F179B}"),
    ("mdi:track-light", "\u{F0914}"),
    ("mdi:spotlight-beam", "\u{F04C9}"),
    ("mdi:lightbulb-spot", "\u{F17F4}"),
    ("mdi:dome-light", "\u{F141E}"),
    ("mdi:vanity-light", "\u{F11E1}"),
    ("mdi:wall-sconce", "\u{F091C}"),
    ("mdi:wall-sconce-flat", "\u{F091D}"),
    ("mdi:wall-sconce-round", "\u{F0748}"),
    ("mdi:coach-lamp", "\u{F1020}"),
    ("mdi:coach-lamp-variant", "\u{F1A37}"),
    ("mdi:outdoor-lamp", "\u{F1054}"),
    ("mdi:post-lamp", "\u{F1A60}"),
    ("mdi:bulkhead-light", "\u{F1A2F}"),
    ("mdi:light-flood-down", "\u{F1987}"),
    ("mdi:light-flood-up", "\u{F1988}"),
    ("mdi:globe-light", "\u{F066F}"),
    ("mdi:lamp", "\u{F06B5}"),
    ("mdi:lamps", "\u{F1576}"),
    ("mdi:desk-lamp", "\u{F095F}"),
    ("mdi:floor-lamp", "\u{F08DD}"),
    ("mdi:floor-lamp-torchiere", "\u{F1747}"),
    ("mdi:lightbulb-fluorescent-tube", "\u{F1804}"),
    ("mdi:led-strip-variant", "\u{F1051}"),
    ("mdi:string-lights", "\u{F12BA}"),
    ("mdi:television-ambient-light", "\u{F1356}"),
    ("mdi:ceiling-fan-light", "\u{F1798}"),
    ("mdi:lightbulb-group", "\u{F1253}"),
];

/// Glyph for a light tile's `mdi:` icon; "" for the default bulb or an
/// unknown name.
pub fn light_glyph(name: &str) -> &'static str {
    LIGHT_ICONS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, g)| *g)
        .unwrap_or("")
}
