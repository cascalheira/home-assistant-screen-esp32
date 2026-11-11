//! Consistency between the firmware, its UI files and the integration.

use std::collections::BTreeSet;
use std::path::Path;

use esp32_screen_host_tests::icons::{light_glyph, page_glyph, LIGHT_ICONS, PAGE_ICONS};

fn repo_file(path: &str) -> String {
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

/// Codepoints written as `\u{XXXX}` escapes in `text`.
fn escapes(text: &str) -> BTreeSet<u32> {
    text.split("\\u{")
        .skip(1)
        .filter_map(|rest| u32::from_str_radix(rest.split('}').next()?, 16).ok())
        .collect()
}

/// Slint embeds only glyphs it sees in string literals: each page and light
/// tile icon must be in the glyph-coverage text.
#[test]
fn icons_are_compiled_into_the_ui() {
    let slint = repo_file("ui/app.slint");
    let start = slint
        .find("if root.glyph-coverage: Text")
        .expect("glyph-coverage text");
    let coverage = escapes(&slint[start..start + slint[start..].find(';').unwrap()]);
    let icons: Vec<_> = PAGE_ICONS.iter().chain(LIGHT_ICONS).collect();
    assert!(icons.len() > 40);
    for (name, glyph) in icons {
        let cp = glyph.chars().next().unwrap() as u32;
        assert!(
            coverage.contains(&cp),
            "{name} (U+{cp:X}) missing from glyph-coverage"
        );
    }
}

/// The `mdi:` names in the tuple or array assigned to `name` in `source`.
fn names_in(source: &str, name: &str, close: char) -> Vec<String> {
    let start = source
        .find(name)
        .and_then(|i| source[i..].find(['(', '[']).map(|j| i + j))
        .unwrap_or_else(|| panic!("{name} not found"));
    let block = &source[start..start + source[start..].find(close).unwrap()];
    block
        .split('"')
        .skip(1)
        .step_by(2)
        .map(String::from)
        .collect()
}

fn names(icons: &[(&str, &str)]) -> Vec<String> {
    icons.iter().map(|(name, _)| name.to_string()).collect()
}

/// The editor offers exactly the icons the firmware can draw, and the
/// integration accepts exactly those.
#[test]
fn icons_match_the_integration() {
    let constants = repo_file("custom_components/esp32_screen/const.py");
    let panel = repo_file("custom_components/esp32_screen/frontend/esp32-screen-panel.js");
    assert!(PAGE_ICONS.len() > 10 && LIGHT_ICONS.len() > 10);
    for (name, icons) in [("PAGE_ICONS", PAGE_ICONS), ("LIGHT_ICONS", LIGHT_ICONS)] {
        assert_eq!(
            names(icons),
            names_in(&constants, &format!("{name}: Final"), ')'),
            "{name} in const.py"
        );
        assert_eq!(
            names(icons),
            names_in(&panel, &format!("const {name} ="), ']'),
            "{name} in the panel"
        );
    }
}

#[test]
fn page_glyph_lookup() {
    assert_eq!(page_glyph("mdi:home"), "\u{F02DC}");
    assert_eq!(page_glyph(""), "");
    assert_eq!(page_glyph("mdi:not-an-icon"), "");
    assert_eq!(light_glyph("mdi:coach-lamp"), "\u{F1020}");
    assert_eq!(light_glyph(""), "");
    assert_eq!(light_glyph("mdi:home"), "");
}
