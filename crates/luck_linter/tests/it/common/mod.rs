use luck_linter::{LintConfig, RuleSetting};

/// The default config with `name` turned on.
pub(crate) fn enable(name: &str) -> LintConfig {
    let mut config = LintConfig::default();
    config
        .rule_overrides
        .insert(name.to_owned(), RuleSetting { enabled: Some(true), severity: None });
    config
}

/// The default config with `name` turned off.
pub(crate) fn disable(name: &str) -> LintConfig {
    let mut config = LintConfig::default();
    config
        .rule_overrides
        .insert(name.to_owned(), RuleSetting { enabled: Some(false), severity: None });
    config
}
