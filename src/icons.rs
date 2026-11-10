//! Material Design Icons allowed as page icons (docs/protocol.md).
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
