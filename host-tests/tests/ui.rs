//! Consistency between the firmware, its UI files and the integration.

use std::collections::BTreeSet;
use std::path::Path;

use esp32_screen_host_tests::icons::{page_glyph, PAGE_ICONS};

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

/// Slint embeds only glyphs it sees in string literals: each page icon must
/// be in the glyph-coverage text.
#[test]
fn page_icons_are_compiled_into_the_ui() {
    let slint = repo_file("ui/app.slint");
    let start = slint
        .find("if root.glyph-coverage: Text")
        .expect("glyph-coverage text");
    let coverage = escapes(&slint[start..start + slint[start..].find(';').unwrap()]);
    assert!(coverage.len() >= PAGE_ICONS.len());
    for (name, glyph) in PAGE_ICONS {
        let cp = glyph.chars().next().unwrap() as u32;
        assert!(
            coverage.contains(&cp),
            "{name} (U+{cp:X}) missing from glyph-coverage"
        );
    }
}

/// The editor offers exactly the icons the firmware can draw.
#[test]
fn page_icons_match_the_integration() {
    let constants = repo_file("custom_components/esp32_screen/const.py");
    let start = constants
        .find("PAGE_ICONS")
        .expect("PAGE_ICONS in const.py");
    let block = &constants[start..start + constants[start..].find(')').unwrap()];
    let python: Vec<&str> = block.split('"').skip(1).step_by(2).collect();
    let firmware: Vec<&str> = PAGE_ICONS.iter().map(|(name, _)| *name).collect();
    assert!(firmware.len() > 10);
    assert_eq!(firmware, python);
}

#[test]
fn page_glyph_lookup() {
    assert_eq!(page_glyph("mdi:home"), "\u{F02DC}");
    assert_eq!(page_glyph(""), "");
    assert_eq!(page_glyph("mdi:not-an-icon"), "");
}
