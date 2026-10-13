fn main() {
    // UI strings use @tr(); translations/<lang>/LC_MESSAGES/caelestia-win.po are
    // compiled into the binary and selected at runtime (general.language).
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/shell.slint", config).unwrap();
}
