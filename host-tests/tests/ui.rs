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

/// Every font size embeds its own copy of the glyphs (flash), so the UI
/// sticks to a fixed set.
#[test]
fn font_sizes_are_from_the_embedded_set() {
    const ALLOWED: [u32; 8] = [14, 16, 18, 20, 24, 28, 40, 48];
    let mut checked = 0;
    for file in [
        "ui/app.slint",
        "ui/dashboard.slint",
        "ui/clock.slint",
        "ui/widgets.slint",
        "ui/theme.slint",
    ] {
        let text = repo_file(file);
        for (line_no, line) in text.lines().enumerate() {
            let Some(rest) = line.split("font-size:").nth(1) else {
                continue;
            };
            for token in rest
                .split(|c: char| !c.is_ascii_digit())
                .filter(|t| !t.is_empty())
            {
                let size: u32 = token.parse().unwrap();
                checked += 1;
                assert!(
                    ALLOWED.contains(&size),
                    "{file}:{}: font size {size}px is not one of {ALLOWED:?}",
                    line_no + 1
                );
            }
        }
    }
    assert!(
        checked > 50,
        "only {checked} font sizes found: is the scan broken?"
    );
}
