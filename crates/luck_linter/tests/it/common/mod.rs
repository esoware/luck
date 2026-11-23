use luck_allocator::Allocator;
use luck_linter::{FixKind, LintConfig, Message, RuleLevel};
use luck_token::LuaTarget;

pub(crate) fn lint(source: &str, target: LuaTarget, config: &LintConfig) -> Vec<Message> {
    luck_linter::lint(&Allocator::new(), source.as_bytes(), target, config, FixKind::empty())
}

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
