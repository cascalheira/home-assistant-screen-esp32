//! Translations: the firmware's own strings and the UI's .po file.

use std::collections::BTreeSet;
use std::path::Path;

use esp32_screen_host_tests::logic::i18n::{set_portuguese, tr, trf, PT};

#[test]
fn rust_strings() {
    set_portuguese(true);
    assert_eq!(tr("Cancel"), "Cancelar");
    assert_eq!(tr("not in the table"), "not in the table");
    assert_eq!(trf("Connecting to {}…", &[&"Casa"]), "A ligar a Casa…");
    assert_eq!(trf("{} signal · {} dBm", &[&tr("Good"), &-60]), "Sinal bom · -60 dBm");
    set_portuguese(false);
    assert_eq!(tr("Cancel"), "Cancel");
    assert_eq!(trf("Connecting to {}…", &[&"Casa"]), "Connecting to Casa…");
}

#[test]
fn table_is_consistent() {
    let mut seen = BTreeSet::new();
    for (en, pt) in PT {
        assert!(seen.insert(*en), "{en:?} is in the table twice");
        assert_eq!(en.matches("{}").count(), pt.matches("{}").count(), "placeholders in {en:?}");
    }
}

/// Every @tr("...") in the .slint files has a Portuguese entry.
#[test]
fn ui_strings_are_translated() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let po = std::fs::read_to_string(root.join("ui/translations/pt/LC_MESSAGES/esp32-screen.po")).unwrap();
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(root.join("ui")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "slint") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for part in text.split("@tr(\"").skip(1) {
            let mut id = String::new();
            let mut chars = part.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        id.push('\\');
                        id.extend(chars.next());
                    }
                    '"' => break,
                    c => id.push(c),
                }
            }
            if !po.contains(&format!("msgid \"{id}\"")) {
                missing.push(id);
            }
        }
    }
    assert!(missing.is_empty(), "not in the .po file: {missing:?}");
}
