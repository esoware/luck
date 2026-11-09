//! `luck.json` project configuration: one module of typed schema per section,
//! file loading and `extends` resolution, build resolution, `.editorconfig`
//! layering, and project file filtering.
//!
//! Every config struct denies unknown fields, and every config type derives
//! `schemars::JsonSchema`. The VS Code schema is generated from
//! [`LuckConfig`], with the type names as its `$defs` keys and the doc
//! comments as its descriptions.

mod dynamic_require;
mod editorconfig;
mod filter;
mod format_config;
mod lint_config;
mod load;
mod luck_config;
mod resolve;
mod transform_config;

pub use dynamic_require::DynamicRequire;
pub use editorconfig::resolved_format_config;
pub use filter::ProjectFilter;
pub use format_config::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatConfig, HexCase, IndentStyle,
    LineEndings, QuoteStyle, SpaceAfterFunction,
};
pub use lint_config::{Category, LintConfig, RuleSetting};
pub use load::{
    ProjectConfig, config_directory, discover_config, load_or_discover_config, load_with_extends,
    parse_luck_config,
};
pub use luck_config::{
    DEFAULT_SEARCH_PATHS, DialectKey, EntryConfig, LuckConfig, ProfileOverrides,
};
pub use resolve::{BuildConfig, resolve_build_config};
pub use transform_config::TransformConfig;

const CONFIG_FILE_NAME: &str = "luck.json";

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const COMMITTED_SCHEMA_PATH: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../editors/vscode/schemas/luckrc.schema.json");

    /// Shared by the regenerator and the drift check so they can't disagree.
    fn luckrc_schema_json() -> String {
        let schema = schemars::schema_for!(LuckConfig);
        let mut pretty = serde_json::to_string_pretty(&schema).expect("schema serializes");
        pretty.push('\n');
        pretty
    }

    #[test]
    #[ignore = "writes the committed schema; run with --ignored to refresh"]
    fn regenerate_luckrc_schema() {
        fs::write(COMMITTED_SCHEMA_PATH, luckrc_schema_json()).expect("write schema");
    }

    #[test]
    fn luckrc_schema_is_up_to_date() {
        // core.autocrlf may have checked the file out with CRLF endings.
        let committed =
            fs::read_to_string(COMMITTED_SCHEMA_PATH).expect("read schema").replace("\r\n", "\n");
        assert_eq!(
            committed,
            luckrc_schema_json(),
            "luckrc.schema.json is stale; regenerate it with \
             `cargo test -p luck_core regenerate_luckrc_schema -- --ignored`"
        );
    }

    #[test]
    fn bug_schema_lists_the_perf_category_alias() {
        parse_luck_config(r#"{ "lint": { "categories": ["perf"] } }"#).expect("`perf` is accepted");
        let schema = serde_json::to_value(schemars::schema_for!(LuckConfig)).expect("serializes");
        let category_values = schema["$defs"]["Category"]["oneOf"].as_array().expect("variants");
        assert!(
            category_values.iter().any(|variant| variant["const"] == "perf"),
            "the schema must accept every value the parser does: {category_values:?}"
        );
    }
}
