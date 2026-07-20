use luck_token::{LuaVersion, StdlibEnvironment};

use crate::LintConfig;
use crate::diagnostic::LintDiagnostic;
use crate::rule::{LintContext, Rule};

/// Parse `source`, run semantic analysis, and check a single rule with the
/// default lint config. Shared by the per-rule test modules so the
/// parse/analyze/context boilerplate lives in exactly one place.
pub(crate) fn run_rule(rule: &dyn Rule, source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
    run_rule_with_config(rule, source, version, &LintConfig::default())
}

/// Like [`run_rule`] but parses as Luau and analyzes with the Roblox
/// stdlib environment, for the Roblox-only rules.
pub(crate) fn run_rule_roblox(rule: &dyn Rule, source: &str) -> Vec<LintDiagnostic> {
    let parse = luck_parser::parse(source, LuaVersion::Luau);
    assert!(parse.errors.is_empty(), "parse: {:?}", parse.errors);
    let semantic = luck_semantic::analyze_with_environment(
        &parse.block,
        LuaVersion::Luau,
        StdlibEnvironment::Roblox,
    );
    let nodes = luck_semantic::nodes::collect_nodes(&parse.block, &semantic.scope_tree);
    rule.check(&LintContext {
        block: &parse.block,
        semantic: &semantic,
        nodes: &nodes,
        source,
        comments: &parse.comments,
        config: &LintConfig::default(),
    })
}

/// Like [`run_rule`] but with a caller-supplied config, for rules whose
/// behavior is driven by config fields (thresholds, restricted paths,
/// extra globals). Mirrors the driver by seeding `config.extra_globals`
/// into the semantic model before checking.
pub(crate) fn run_rule_with_config(
    rule: &dyn Rule,
    source: &str,
    version: LuaVersion,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    let parse = luck_parser::parse(source, version);
    assert!(parse.errors.is_empty(), "parse: {:?}", parse.errors);
    let mut semantic = luck_semantic::analyze(&parse.block, version);
    for name in &config.extra_globals {
        semantic.extra_globals.insert(name.clone());
    }
    let nodes = luck_semantic::nodes::collect_nodes(&parse.block, &semantic.scope_tree);
    rule.check(&LintContext {
        block: &parse.block,
        semantic: &semantic,
        nodes: &nodes,
        source,
        comments: &parse.comments,
        config,
    })
}
