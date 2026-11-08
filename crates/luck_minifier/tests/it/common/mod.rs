use luck_core::TransformConfig;
use luck_minifier::minify;
use luck_token::LuaVersion;

/// Minifies `source` for `version` with `config`, which must succeed.
pub(crate) fn minify_with(source: &str, version: LuaVersion, config: &TransformConfig) -> String {
    minify(source, version, config, "<test>").expect("minify failed")
}

pub(crate) fn minify_lua54(source: &str) -> String {
    minify_with(source, LuaVersion::Lua54, &TransformConfig::default())
}

pub(crate) fn minify_luau(source: &str) -> String {
    minify_with(source, LuaVersion::Luau, &TransformConfig::default())
}

/// Minifies with the opt-in `rename_globals` on.
pub(crate) fn minify_renaming_globals(source: &str, version: LuaVersion) -> String {
    let config = TransformConfig { rename_globals: true, ..TransformConfig::default() };
    minify_with(source, version, &config)
}

/// Whether `source` parses clean as Lua 5.4.
pub(crate) fn reparses(source: &str) -> bool {
    luck_parser::parse(source, LuaVersion::Lua54).errors.is_empty()
}
