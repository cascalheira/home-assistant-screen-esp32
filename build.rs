fn main() {
    embuild::espidf::sysenv::output();

    // Art drawn at this board's pixel size: "@board" in the .slint files.
    // MCU is set by scripts/cargo-esp.sh (and .cargo/config.toml).
    println!("cargo:rerun-if-env-changed=MCU");
    let mcu = std::env::var("MCU").unwrap_or_else(|_| "esp32s3".into());
    let board = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("ui/board")
        .join(&mcu)
        .join("board.slint");

    let config = slint_build::CompilerConfiguration::new()
        // Pre-render the glyphs of the bundled fonts into the binary; the
        // device has no font files or font rasterizer at runtime.
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer)
        // One distance field per glyph, scaled to every font size, instead of
        // a bitmap per glyph and size.
        .with_sdf_fonts(true)
        // European Portuguese (ui/translations/pt), picked at runtime.
        .with_bundled_translations("ui/translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None)
        .with_library_paths(std::collections::HashMap::from([(
            "board".to_string(),
            board,
        )]));
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
