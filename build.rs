fn main() {
    embuild::espidf::sysenv::output();

    let config = slint_build::CompilerConfiguration::new()
        // Pre-render the glyphs of the bundled fonts into the binary; the
        // device has no font files or font rasterizer at runtime.
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
