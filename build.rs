fn main() {
    embuild::espidf::sysenv::output();

    let config = slint_build::CompilerConfiguration::new()
        // Pre-render the glyphs of the bundled fonts into the binary; the
        // device has no font files or font rasterizer at runtime.
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer)
        // One distance field per glyph, scaled to every font size, instead of
        // a bitmap per glyph and size.
        .with_sdf_fonts(true)
        // European Portuguese (ui/translations/pt), picked at runtime.
        .with_bundled_translations("ui/translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
