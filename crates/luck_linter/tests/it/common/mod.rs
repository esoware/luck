use luck_linter::{LintConfig, RuleSetting};

pub(crate) fn enable(name: &str) -> LintConfig {
    let mut config = LintConfig::default();
    config
        .rule_overrides
        .insert(name.to_owned(), RuleSetting { enabled: Some(true), severity: None });
    config
}

pub(crate) fn disable(name: &str) -> LintConfig {
    let mut config = LintConfig::default();
    config
        .rule_overrides
        .insert(name.to_owned(), RuleSetting { enabled: Some(false), severity: None });
    config
}
