//! `luck.json` project configuration: the typed schema and its `extends`
//! merging, file loading and discovery, build resolution, `.editorconfig`
//! layering, and project file filtering.

mod editorconfig;
mod filter;
mod load;
mod resolve;
mod schema;

pub use editorconfig::resolved_format_config;
pub use filter::ProjectFilter;
pub use load::{
    ProjectConfig, discover_config, load_or_discover_config, load_with_extends, parse_luck_config,
};
pub use resolve::{BuildConfig, resolve_build_config};
pub use schema::{
    Category, DEFAULT_SEARCH_PATHS, DialectKey, EntryConfig, FormatConfig, LintConfig, LuckConfig,
    ProfileOverrides, RuleSetting,
};

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
}
