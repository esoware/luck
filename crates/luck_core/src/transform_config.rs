use serde::Deserialize;

/// Per-transform toggle flags for the minification pipeline.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
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
    /// Rename globals DEFINED in this file (`function myHelper() end`,
    /// `counter = 0`). Off by default: renamed globals live under
    /// different `_G` keys, breaking any cross-chunk consumer,
    /// `_G["name"]` access, or loaded string that expects the original
    /// names. Enable only for fully self-contained single-chunk scripts.
    pub rename_globals: bool,
}

impl Default for TransformConfig {
    fn default() -> Self {
        Self {
            remove_dead_code: true,
            simplify_statements: true,
            fold_constants: true,
            inline_locals: true,
            merge_locals: true,
            simplify_indexes: true,
            shorten_strings: true,
            shorten_numbers: true,
            simplify_parens: true,
            rename_locals: true,
            lift_locals: true,
            rename_globals: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_all_enabled() {
        let config = TransformConfig::default();
        assert!(config.fold_constants);
        assert!(config.rename_locals);
    }

    #[test]
    fn deserialize_partial() {
        let json = r#"{"fold_constants": false}"#;
        let config: TransformConfig = serde_json::from_str(json).expect("deserialize failed");
        assert!(!config.fold_constants);
        assert!(config.rename_locals);
    }
}
