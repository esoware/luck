//! # luck_linter
//!
//! Lua/Luau linter with scope-aware analysis, auto-fix support, and suppression comments.
//!
//! Uses `luck_semantic` for scope analysis and `luck_parser` for parsing.
//! Rules are organized by category: correctness, suspicious, style, performance.
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//! use luck_linter::LintConfig;
//!
//! let diagnostics = luck_linter::lint("print(1)", LuaVersion::Lua54, &LintConfig::default());
//! assert!(diagnostics.is_empty());
//! ```

pub mod bus;
mod cfg;
pub mod diagnostic;
pub mod fix;
pub mod format_pattern;
mod path;
mod roblox;
pub mod rule;
pub mod rules;
mod suggest;
pub mod suppression;

#[cfg(test)]
pub(crate) mod test_support;

pub use fix::{FIXPOINT_BUDGET, FixpointError, apply_fixes, apply_fixes_fixpoint};

use diagnostic::{Category, LintDiagnostic, Severity};
use luck_ast::node::NodeKind;
use luck_semantic::nodes::Nodes;
use luck_token::{LuaVersion, StdlibEnvironment};

/// The lint configuration types live in `luck_core` as the single source of
/// truth and are re-exported here so rule code and external callers keep using
/// the `luck_linter::{LintConfig, RuleSetting}` paths.
pub use luck_core::{LintConfig, RuleSetting};

/// Returns the lint config's rule-override names that are not registered
/// rules. Uses the same registered-name source as `invalid_lint_filter`,
/// so config-file and CLI-supplied rule names are validated against the
/// exact set the linter actually runs. The result is sorted for stable
/// error output.
pub fn unknown_rule_names(config: &LintConfig) -> Vec<String> {
    let known = rules::registered_rule_names();
    let mut unknown: Vec<String> = config
        .rule_overrides
        .keys()
        .filter(|name| !known.contains(&name.as_str()))
        .cloned()
        .collect();
    unknown.sort();
    unknown
}

/// Lint Lua source code, returning all diagnostics. Defaults to the
/// [`StdlibEnvironment::Standalone`] environment, the correct default for
/// vanilla Lua and standalone Luau. Roblox callers must go through
/// [`lint_target`] with a Roblox `LuaTarget`.
pub fn lint(source: &str, version: LuaVersion, config: &LintConfig) -> Vec<LintDiagnostic> {
    lint_inner(source, version, StdlibEnvironment::Standalone, config)
}

/// Lint using a full `LuaTarget`, which carries the stdlib environment so
/// that Roblox vs standalone globals resolve correctly.
pub fn lint_target(
    source: &str,
    target: luck_core::LuaTarget,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    lint_inner(
        source,
        target.lua_version(),
        target.stdlib_environment(),
        config,
    )
}

fn lint_inner(
    source: &str,
    version: LuaVersion,
    environment: StdlibEnvironment,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    let parse_result = luck_parser::parse(source, version);
    lint_parsed(&parse_result, version, environment, config)
}

/// Lint an ALREADY-PARSED document. Long-lived hosts (the LSP) parse once
/// per edit and cache the result: re-parsing inside the linter doubled
/// (sometimes tripled) the per-keystroke parse cost.
pub fn lint_parsed(
    parse_result: &luck_parser::ParseResult,
    version: LuaVersion,
    environment: StdlibEnvironment,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    let source = parse_result.source.as_str();

    if !parse_result.errors.is_empty() {
        return parse_result
            .errors
            .iter()
            .map(|e| LintDiagnostic {
                rule: "parse_error",
                category: Category::Correctness,
                severity: Severity::Error,
                message: e.message.clone(),
                span: e.span,
                help: None,
                fix: None,
            })
            .collect();
    }

    let mut semantic =
        luck_semantic::analyze_with_environment(&parse_result.block, version, environment);

    for name in &config.extra_globals {
        semantic.extra_globals.insert(name.clone());
    }

    let nodes = luck_semantic::nodes::collect_nodes(&parse_result.block, &semantic.scope_tree);

    let statement_spans = collect_statement_spans(&nodes);
    let suppression =
        suppression::Suppression::from_comments(&parse_result.comments, source, &statement_spans);

    let lint_ctx = rule::LintContext {
        block: &parse_result.block,
        semantic: &semantic,
        nodes: &nodes,
        source,
        comments: &parse_result.comments,
        config,
    };

    // Node-local rules share ONE pass over the node table (the bus);
    // whole-tree rules fan out across cores in parallel with it. Results
    // land in per-registry-slot vecs, so concatenation order, and
    // therefore stable-sort tie order, is identical to a sequential loop
    // over the registry. A node rule whose declared types are absent from
    // this file is dropped before dispatch.
    use rayon::prelude::*;
    let mut whole_rules: Vec<(usize, &'static dyn rule::Rule)> = Vec::new();
    let mut node_rules: Vec<(usize, &'static dyn rule::NodeRule)> = Vec::new();
    #[cfg(debug_assertions)]
    let mut skipped_node_rules: Vec<&'static dyn rule::NodeRule> = Vec::new();
    for (slot, entry) in rules::RULES.iter().enumerate() {
        if !is_rule_enabled(entry.rule(), config) {
            continue;
        }
        match entry {
            rules::RuleEntry::Whole(whole) => whole_rules.push((slot, *whole)),
            rules::RuleEntry::Node(_, node) => {
                if let Some(types) = node.node_types()
                    && !nodes.contains_any(types)
                {
                    #[cfg(debug_assertions)]
                    skipped_node_rules.push(*node);
                    continue;
                }
                node_rules.push((slot, *node));
            }
        }
    }

    let node_rule_refs: Vec<&dyn rule::NodeRule> =
        node_rules.iter().map(|(_, node)| *node).collect();
    let (bus_results, whole_results) = rayon::join(
        || bus::run(&node_rule_refs, &lint_ctx),
        || {
            whole_rules
                .par_iter()
                .map(|(slot, whole)| (*slot, whole.check(&lint_ctx)))
                .collect::<Vec<_>>()
        },
    );

    // Bucketed dispatch and the file-level skip must be pure
    // optimizations: re-run every node rule against every node and
    // require identical diagnostics. A mismatch means a rule's
    // `node_types()` is missing a type its hooks act on.
    #[cfg(debug_assertions)]
    {
        let reference_results = bus::run_every_node(&node_rule_refs, &lint_ctx);
        for ((_, rule), (bucketed, reference)) in node_rules
            .iter()
            .zip(bus_results.iter().zip(&reference_results))
        {
            assert_eq!(
                bucketed,
                reference,
                "rule '{}': bucketed dispatch diverged from every-node dispatch; \
                 its node_types() declaration is missing a type",
                rule.name()
            );
        }
        for (rule, reference) in skipped_node_rules
            .iter()
            .zip(bus::run_every_node(&skipped_node_rules, &lint_ctx))
        {
            assert!(
                reference.is_empty(),
                "rule '{}' was skipped for this file but produces diagnostics; \
                 its node_types() declaration is missing a type",
                rule.name()
            );
        }
    }

    let mut per_slot: Vec<Vec<LintDiagnostic>> = vec![Vec::new(); rules::RULES.len()];
    for ((slot, _), diags) in node_rules.iter().zip(bus_results) {
        per_slot[*slot] = diags;
    }
    for (slot, diags) in whole_results {
        per_slot[slot] = diags;
    }

    let mut diagnostics: Vec<LintDiagnostic> = Vec::new();
    for (slot, mut rule_diags) in per_slot.into_iter().enumerate() {
        if rule_diags.is_empty() {
            continue;
        }
        let rule = rules::RULES[slot].rule();
        let severity = config
            .rule_overrides
            .get(rule.name())
            .and_then(|setting| setting.severity)
            .unwrap_or(rule.default_severity());
        // The rule's `category()` is authoritative: it both gates
        // enablement and now stamps every diagnostic, so a per-diagnostic
        // `category:` literal can't disagree with the rule's category.
        let category = rule.category();
        for diag in &mut rule_diags {
            diag.severity = severity;
            diag.category = category;
        }
        diagnostics.extend(rule_diags);
    }

    suppression.apply(&mut diagnostics);

    diagnostics.sort_by_key(|d| d.span.start);
    diagnostics
}

/// Statement spans for suppression resolution: one linear pass over the
/// node table. Suppression expects the list sorted by start offset.
fn collect_statement_spans(nodes: &Nodes) -> Vec<(u32, u32)> {
    let mut spans: Vec<(u32, u32)> = nodes
        .iter()
        .filter_map(|node| match node.kind {
            NodeKind::Statement(stmt) => Some(stmt.span()),
            NodeKind::LastStatement(last) => Some(last.span()),
            NodeKind::Expression(_) => None,
        })
        .map(|span| (span.start, span.end))
        .collect();
    spans.sort_unstable_by_key(|(start, _)| *start);
    spans
}

/// Decide whether a rule runs for this lint pass, applying both the
/// per-rule override and the default-rule kill-switch.
fn is_rule_enabled(rule: &dyn rule::Rule, config: &LintConfig) -> bool {
    if let Some(setting) = config.rule_overrides.get(rule.name())
        && let Some(enabled) = setting.enabled
    {
        return enabled;
    }
    if config.disable_default_rules {
        return false;
    }
    rule.category() == Category::Correctness || config.categories.contains(&rule.category())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enable(name: &str) -> LintConfig {
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            name.to_string(),
            RuleSetting {
                enabled: Some(true),
                severity: None,
            },
        );
        config
    }

    fn disable(name: &str) -> LintConfig {
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            name.to_string(),
            RuleSetting {
                enabled: Some(false),
                severity: None,
            },
        );
        config
    }

    #[test]
    fn unknown_rule_names_flags_only_unregistered() {
        let mut config = LintConfig::default();
        config
            .rule_overrides
            .insert("unused_variable".to_string(), RuleSetting::default());
        config
            .rule_overrides
            .insert("not_a_real_rule".to_string(), RuleSetting::default());
        assert_eq!(unknown_rule_names(&config), vec!["not_a_real_rule"]);
    }

    #[test]
    fn lint_target_distinguishes_luau_flavors() {
        use luck_core::LuaTarget;
        let standalone = lint_target("print(game)", LuaTarget::Luau, &LintConfig::default());
        assert!(
            standalone
                .iter()
                .any(|d| d.rule == "undefined_variable" && d.message.contains("game"))
        );
        let roblox = lint_target("print(game)", LuaTarget::LuauRoblox, &LintConfig::default());
        assert!(roblox.iter().all(|d| d.rule != "undefined_variable"));
    }

    #[test]
    fn detects_undefined_variable() {
        let diags = lint(
            "print(undefined_var)",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "undefined_variable" && d.message.contains("undefined_var"))
        );
    }

    #[test]
    fn named_vararg_not_flagged_undefined() {
        // Lua 5.5 `...name` declares a binding; its uses are not globals.
        let diags = lint(
            "local function f(...args) return args end\nf(1)",
            LuaVersion::Lua55,
            &LintConfig::default(),
        );
        assert!(
            diags.iter().all(|d| d.rule != "undefined_variable"),
            "{diags:?}"
        );
    }

    #[test]
    fn known_globals_not_flagged() {
        let diags = lint(
            "print(tostring(1))",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(diags.iter().all(|d| d.rule != "undefined_variable"));
    }

    #[test]
    fn detects_unused_variable() {
        let diags = lint(
            "local unused = 1",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "unused_variable" && d.message.contains("unused"))
        );
    }

    #[test]
    fn underscore_prefix_skipped() {
        let diags = lint(
            "local _unused = 1",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(diags.iter().all(|d| d.rule != "unused_variable"));
    }

    #[test]
    fn detects_setting_global() {
        let diags = lint("my_global = 1", LuaVersion::Lua54, &LintConfig::default());
        assert!(diags.iter().any(|d| d.rule == "setting_global"));
    }

    #[test]
    fn detects_duplicate_keys() {
        let diags = lint(
            "local t = { a = 1, a = 2 }",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(diags.iter().any(|d| d.rule == "duplicate_keys"));
    }

    #[test]
    fn extra_globals_not_flagged() {
        let config = LintConfig {
            extra_globals: vec!["vim".to_string()],
            ..Default::default()
        };
        let diags = lint("print(vim.fn)", LuaVersion::Lua54, &config);
        assert!(
            diags
                .iter()
                .all(|d| d.rule != "undefined_variable" || !d.message.contains("vim"))
        );
    }

    #[test]
    fn suppression_works() {
        let diags = lint(
            "-- luck: allow(unused_variable)\nlocal unused = 1",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(diags.iter().all(|d| d.rule != "unused_variable"));
    }

    #[test]
    fn rule_override_disables() {
        let config = disable("unused_variable");
        let diags = lint("local unused = 1", LuaVersion::Lua54, &config);
        assert!(diags.iter().all(|d| d.rule != "unused_variable"));
    }

    #[test]
    fn categories_enable_group() {
        let config = LintConfig {
            categories: vec![diagnostic::Category::Suspicious],
            ..Default::default()
        };
        let diags = lint("if true then end", LuaVersion::Lua54, &config);
        assert!(diags.iter().any(|d| d.rule == "empty_block"));
    }

    #[test]
    fn suspicious_rules_off_by_default() {
        let diags = lint(
            "if true then end",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(diags.iter().all(|d| d.rule != "empty_block"));
    }

    #[test]
    fn suspicious_rules_enabled_by_override() {
        let config = enable("empty_block");
        let diags = lint("if true then end", LuaVersion::Lua54, &config);
        assert!(diags.iter().any(|d| d.rule == "empty_block"));
    }

    #[test]
    fn detects_compare_nan() {
        let diags = lint(
            "local x = 1\nif x == 0/0 then end",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(diags.iter().any(|d| d.rule == "compare_nan"));
    }

    #[test]
    fn detects_constant_table_comparison() {
        let diags = lint(
            "local x = {}\nif x == {} then end",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(diags.iter().any(|d| d.rule == "constant_table_comparison"));
    }

    #[test]
    fn detects_almost_swapped() {
        let diags = lint(
            "local a, b = 1, 2\na = b\nb = a",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(diags.iter().any(|d| d.rule == "almost_swapped"));
    }

    #[test]
    fn detects_type_check_inside_call() {
        let diags = lint(
            "if type(x == \"string\") then end",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(diags.iter().any(|d| d.rule == "type_check_inside_call"));
    }

    #[test]
    fn detects_reversed_for_loop() {
        let config = enable("reversed_for_loop");
        let diags = lint("for i = 10, 1 do end", LuaVersion::Lua54, &config);
        assert!(diags.iter().any(|d| d.rule == "reversed_for_loop"));
    }

    #[test]
    fn detects_unbalanced_assignment() {
        let config = enable("unbalanced_assignment");
        let diags = lint("local a, b, c = 1, 2", LuaVersion::Lua54, &config);
        assert!(diags.iter().any(|d| d.rule == "unbalanced_assignment"));
    }

    #[test]
    fn detects_incorrect_stdlib_use_too_few() {
        let diags = lint("table.insert()", LuaVersion::Lua54, &LintConfig::default());
        assert!(diags.iter().any(|d| d.rule == "incorrect_stdlib_use"));
    }

    #[test]
    fn detects_must_use() {
        let config = enable("must_use");
        let diags = lint("tostring(42)", LuaVersion::Lua54, &config);
        assert!(diags.iter().any(|d| d.rule == "must_use"));
    }

    #[test]
    fn deny_escalates_severity() {
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            "undefined_variable".to_string(),
            RuleSetting {
                enabled: Some(true),
                severity: Some(Severity::Warning),
            },
        );
        let diags = lint(
            "-- luck: deny(undefined_variable)\nprint(undef)",
            LuaVersion::Lua54,
            &config,
        );
        let denied = diags.iter().find(|d| d.rule == "undefined_variable");
        assert!(denied.is_some());
        assert_eq!(denied.unwrap().severity, Severity::Error);
    }

    #[test]
    fn warn_downgrades_severity() {
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            "undefined_variable".to_string(),
            RuleSetting {
                enabled: Some(true),
                severity: Some(Severity::Error),
            },
        );
        let diags = lint(
            "-- luck: warn(undefined_variable)\nprint(undef)",
            LuaVersion::Lua54,
            &config,
        );
        let down = diags.iter().find(|d| d.rule == "undefined_variable");
        assert!(down.is_some());
        assert_eq!(down.unwrap().severity, Severity::Warning);
    }

    #[test]
    fn file_level_allow_covers_whole_file() {
        let code = "-- #luck: allow(unused_variable)\nlocal a = 1\nlocal b = 2";
        let diags = lint(code, LuaVersion::Lua54, &LintConfig::default());
        assert!(diags.iter().all(|d| d.rule != "unused_variable"));
    }

    #[test]
    fn deny_region_covers_block() {
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            "unused_variable".to_string(),
            RuleSetting {
                enabled: Some(true),
                severity: Some(Severity::Warning),
            },
        );
        let code = "-- luck: deny(unused_variable) start\nlocal a = 1\nlocal b = 2\n-- luck: deny(unused_variable) end";
        let diags = lint(code, LuaVersion::Lua54, &config);
        let in_region: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "unused_variable")
            .collect();
        assert!(!in_region.is_empty());
        for diag in in_region {
            assert_eq!(diag.severity, Severity::Error);
        }
    }

    #[test]
    fn unclosed_region_extends_to_end_of_file() {
        let code = "-- luck: allow(unused_variable) start\nlocal a = 1\nlocal b = 2";
        let diags = lint(code, LuaVersion::Lua54, &LintConfig::default());
        assert!(diags.iter().all(|d| d.rule != "unused_variable"));
    }

    #[test]
    fn region_suppression_works() {
        let code = "-- luck: allow(unused_variable) start\nlocal a = 1\nlocal b = 2\n-- luck: allow(unused_variable) end\nlocal c = 3";
        let diags = lint(code, LuaVersion::Lua54, &LintConfig::default());
        assert!(
            diags
                .iter()
                .all(|d| !(d.rule == "unused_variable" && d.message.contains("`a`")))
        );
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "unused_variable" && d.message.contains("`c`"))
        );
    }

    #[test]
    fn invalid_lint_filter_fires_on_unknown_name() {
        let code = "-- luck: allow(unsed_variable)\nlocal _x = 1";
        let diags = lint(code, LuaVersion::Lua54, &LintConfig::default());
        let invalid = diags.iter().find(|d| d.rule == "invalid_lint_filter");
        assert!(invalid.is_some(), "diags: {diags:?}");
        assert!(invalid.unwrap().message.contains("unused_variable"));
    }

    #[test]
    fn invalid_lint_filter_silent_on_known() {
        let code = "-- luck: allow(unused_variable)\nlocal _x = 1";
        let diags = lint(code, LuaVersion::Lua54, &LintConfig::default());
        assert!(diags.iter().all(|d| d.rule != "invalid_lint_filter"));
    }

    #[test]
    fn wildcard_suppression_still_works() {
        let code = "-- luck: allow(*)\nlocal unused = 1\nmy_global = 1";
        let diags = lint(code, LuaVersion::Lua54, &LintConfig::default());
        assert!(
            diags
                .iter()
                .all(|d| !(d.rule == "unused_variable" && d.message.contains("unused")))
        );
        assert!(diags.iter().any(|d| d.rule == "setting_global"));
    }

    #[test]
    fn unused_variable_fix_prefixes_underscore() {
        let source = "local unused = 1";
        let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
        let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local _unused = 1");
        let reparsed = luck_parser::parse(&fixed, LuaVersion::Lua54);
        assert!(reparsed.errors.is_empty());
    }

    #[test]
    fn parenthesized_conditions_fix_removes_parens() {
        let config = enable("parenthesized_conditions");
        let source = "local x = 1\nif (x == 1) then end";
        let diags = lint(source, LuaVersion::Lua54, &config);
        let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
        assert!(fixed.contains("if x == 1 then"));
    }

    #[test]
    fn deprecated_loadstring_fix() {
        let source = "local f = loadstring('return 1')";
        let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
        let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
        assert!(fixed.contains("load("));
        assert!(!fixed.contains("loadstring"));
    }

    #[test]
    fn no_fix_when_already_underscored() {
        let source = "local _unused = 1";
        let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
        let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, source);
    }

    #[test]
    fn disable_default_rules_silences_correctness() {
        let config = LintConfig {
            disable_default_rules: true,
            ..Default::default()
        };
        let diags = lint("print(undefined_var)", LuaVersion::Lua54, &config);
        assert!(diags.is_empty());
    }

    #[test]
    fn cyclomatic_complexity_fires_when_threshold_configured() {
        let mut config = LintConfig {
            max_cyclomatic_complexity: Some(2),
            ..Default::default()
        };
        config.rule_overrides.insert(
            "cyclomatic_complexity".to_string(),
            RuleSetting {
                enabled: Some(true),
                severity: None,
            },
        );
        let source = "function f() if a then end if b then end if c then end if d then end end";
        let diags = lint(source, LuaVersion::Lua54, &config);
        assert!(
            diags.iter().any(|d| d.rule == "cyclomatic_complexity"),
            "expected cyclomatic_complexity to fire; got: {diags:?}"
        );
    }

    #[test]
    fn cyclomatic_complexity_inert_without_threshold() {
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            "cyclomatic_complexity".to_string(),
            RuleSetting {
                enabled: Some(true),
                severity: None,
            },
        );
        let source = "function f() if a then end if b then end if c then end if d then end end";
        let diags = lint(source, LuaVersion::Lua54, &config);
        assert!(
            diags.iter().all(|d| d.rule != "cyclomatic_complexity"),
            "expected no cyclomatic_complexity diagnostic when threshold is None; got: {diags:?}"
        );
    }

    #[test]
    fn restricted_module_paths_fires_when_configured() {
        let config = LintConfig {
            restricted_module_paths: vec!["forbidden.lib".to_string()],
            ..Default::default()
        };
        let diags = lint("require(\"forbidden.lib\")", LuaVersion::Lua54, &config);
        assert!(
            diags.iter().any(|d| d.rule == "restricted_module_paths"),
            "expected restricted_module_paths to fire; got: {diags:?}"
        );
    }

    #[test]
    fn restricted_module_paths_inert_with_empty_list() {
        let diags = lint(
            "require(\"anything\")",
            LuaVersion::Lua54,
            &LintConfig::default(),
        );
        assert!(
            diags.iter().all(|d| d.rule != "restricted_module_paths"),
            "expected no restricted_module_paths diagnostic with empty list; got: {diags:?}"
        );
    }

    #[test]
    fn disable_default_rules_still_honors_explicit_enable() {
        let mut config = LintConfig {
            disable_default_rules: true,
            ..Default::default()
        };
        config.rule_overrides.insert(
            "undefined_variable".to_string(),
            RuleSetting {
                enabled: Some(true),
                severity: None,
            },
        );
        let diags = lint("print(undefined_var)", LuaVersion::Lua54, &config);
        assert!(diags.iter().any(|d| d.rule == "undefined_variable"));
    }
}
