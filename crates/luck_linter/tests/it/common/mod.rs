use luck_linter::{LintConfig, RuleLevel};

pub(crate) fn with_level(name: &str, level: RuleLevel) -> LintConfig {
    let mut config = LintConfig::default();
    config.rules.set_level(name, level);
    config
}

pub(crate) fn enable(name: &str) -> LintConfig {
    with_level(name, RuleLevel::Warn)
}

pub(crate) fn disable(name: &str) -> LintConfig {
    with_level(name, RuleLevel::Off)
}
