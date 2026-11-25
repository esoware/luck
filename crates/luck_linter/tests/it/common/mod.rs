use luck_allocator::Allocator;
use luck_linter::{FixKind, LintConfig, Linter, Message, RuleId, RuleLevel};
use luck_stdlib::library_for;
use luck_token::Variant;

/// `source`'s messages against Luau's standard library, with no fixes.
pub(crate) fn lint(source: &str, config: &LintConfig) -> Vec<Message> {
    lint_bytes(source.as_bytes(), Variant::Standalone, config, FixKind::empty())
}

/// `source`'s messages against `variant`'s standard library, with the fixes
/// of the kinds `fixes` holds.
pub(crate) fn lint_bytes(
    source: &[u8],
    variant: Variant,
    config: &LintConfig,
    fixes: FixKind,
) -> Vec<Message> {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, source);
    Linter::new(config.clone()).run(&parse, library_for(variant), fixes, None)
}

pub(crate) fn with_level(rule: RuleId, level: RuleLevel) -> LintConfig {
    let mut config = LintConfig::default();
    config.rules.set_level(rule, level);
    config
}

pub(crate) fn enable(rule: RuleId) -> LintConfig {
    with_level(rule, RuleLevel::Warn)
}

pub(crate) fn disable(rule: RuleId) -> LintConfig {
    with_level(rule, RuleLevel::Off)
}
