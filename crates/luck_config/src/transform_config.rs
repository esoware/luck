/// Turns individual minifier passes on or off. Every pass except
/// `rename_globals` is on by default.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent key of the transforms object in luck.json"
)]
pub struct TransformConfig {
    pub remove_dead_code: bool,
    pub simplify_statements: bool,
    pub fold_constants: bool,
    pub inline_locals: bool,
    pub merge_locals: bool,
    pub simplify_indexes: bool,
    pub shorten_strings: bool,
    pub shorten_numbers: bool,
    pub simplify_parens: bool,
    pub rename_locals: bool,
    pub lift_locals: bool,
    /// Rename globals this file defines, such as `function helper() end` or
    /// `counter = 0`. Off by default: renamed globals land under different
    /// `_G` keys, which breaks other chunks, `_G["name"]` lookups, and loaded
    /// strings that use the original names. Only takes effect with
    /// `rename_locals` on.
    pub rename_globals: bool,
}

/// The minifier's defaults, so a key left out keeps them.
impl Default for TransformConfig {
    fn default() -> Self {
        let luck_minifier::TransformConfig {
            remove_dead_code,
            simplify_statements,
            fold_constants,
            inline_locals,
            merge_locals,
            simplify_indexes,
            shorten_strings,
            shorten_numbers,
            simplify_parens,
            rename_locals,
            lift_locals,
            rename_globals,
        } = luck_minifier::TransformConfig::default();
        Self {
            remove_dead_code,
            simplify_statements,
            fold_constants,
            inline_locals,
            merge_locals,
            simplify_indexes,
            shorten_strings,
            shorten_numbers,
            simplify_parens,
            rename_locals,
            lift_locals,
            rename_globals,
        }
    }
}

impl From<TransformConfig> for luck_minifier::TransformConfig {
    fn from(config: TransformConfig) -> Self {
        let TransformConfig {
            remove_dead_code,
            simplify_statements,
            fold_constants,
            inline_locals,
            merge_locals,
            simplify_indexes,
            shorten_strings,
            shorten_numbers,
            simplify_parens,
            rename_locals,
            lift_locals,
            rename_globals,
        } = config;
        Self {
            remove_dead_code,
            simplify_statements,
            fold_constants,
            inline_locals,
            merge_locals,
            simplify_indexes,
            shorten_strings,
            shorten_numbers,
            simplify_parens,
            rename_locals,
            lift_locals,
            rename_globals,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_keys_keep_their_defaults() {
        let json = r#"{"fold_constants": false}"#;
        let config: TransformConfig = serde_json::from_str(json).expect("deserialize failed");
        assert!(!config.fold_constants);
        assert!(config.rename_locals);
        assert!(!config.rename_globals);
    }
}
