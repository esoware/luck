use luck_allocator::Allocator;
use luck_minifier::{TransformConfig, minify};

/// Minifies `source`, and checks that the output reparses and minifies to
/// itself.
pub(crate) fn minify_bytes(source: &[u8], config: &TransformConfig) -> Vec<u8> {
    let allocator = Allocator::new();
    let output = minify(&allocator, source, config).expect("minify failed");
    let again = minify(&allocator, &output, config).expect("the output reparses");
    assert_eq!(again, output, "minifying the output again changed it\n{}", source.escape_ascii());
    output
}

/// [`minify_bytes`] of text, which minifies as text.
pub(crate) fn minify_with(source: &str, config: &TransformConfig) -> String {
    String::from_utf8(minify_bytes(source.as_bytes(), config))
        .expect("a source that's UTF-8 minifies as UTF-8")
}

pub(crate) fn minify_default(source: &str) -> String {
    minify_with(source, &TransformConfig::default())
}

/// [`minify_default`] keeping the types, as a typed library minifies.
pub(crate) fn minify_typed(source: &str) -> String {
    let config = TransformConfig { strip_types: false, ..TransformConfig::default() };
    minify_with(source, &config)
}

pub(crate) fn minify_renaming_globals(source: &str) -> String {
    let config = TransformConfig { rename_globals: true, ..TransformConfig::default() };
    minify_with(source, &config)
}

pub(crate) fn reparses(source: &str) -> bool {
    let allocator = Allocator::new();
    luck_parser::parse(&allocator, source.as_bytes()).errors.is_empty()
}
