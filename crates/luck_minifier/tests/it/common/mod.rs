use luck_allocator::Allocator;
use luck_minifier::{TransformConfig, minify};
use luck_token::LuaVersion;

/// Minifies `source`, and checks that the output reparses and minifies to
/// itself.
pub(crate) fn minify_with(source: &str, version: LuaVersion, config: &TransformConfig) -> String {
    let allocator = Allocator::new();
    let output = minify(&allocator, source, version, config).expect("minify failed");
    let again = minify(&allocator, &output, version, config).expect("the output reparses");
    assert_eq!(again, output, "minifying the output again changed it\n{source}");
    output
}

pub(crate) fn minify_lua54(source: &str) -> String {
    minify_with(source, LuaVersion::Lua54, &TransformConfig::default())
}

pub(crate) fn minify_luau(source: &str) -> String {
    minify_with(source, LuaVersion::Luau, &TransformConfig::default())
}

pub(crate) fn minify_renaming_globals(source: &str, version: LuaVersion) -> String {
    let config = TransformConfig { rename_globals: true, ..TransformConfig::default() };
    minify_with(source, version, &config)
}

pub(crate) fn reparses(source: &str) -> bool {
    let allocator = Allocator::new();
    luck_parser::parse(&allocator, source, LuaVersion::Lua54).errors.is_empty()
}
