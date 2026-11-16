//! Every rule walks the tree, and the driver runs them on rayon's threads, so
//! those run on the smallest stack too.

use luck_linter::{LintConfig, apply_fixes, lint_parsed};
use luck_testgen::{SMALLEST_STACK_BYTES, deep_and_long_sources, on_the_smallest_stack};
use luck_token::StdlibEnvironment;
use rayon::ThreadPoolBuilder;

fn every_rule() -> LintConfig {
    serde_json::from_str(
        r#"{
            "categories": {
                "correctness": "warn",
                "suspicious": "warn",
                "style": "warn",
                "performance": "warn"
            },
            "rules": { "cyclomatic_complexity": ["warn", { "max": 1 }] }
        }"#,
    )
    .expect("the config parses")
}

#[test]
fn linting_and_fixing_fit_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let pool = ThreadPoolBuilder::new()
            .stack_size(SMALLEST_STACK_BYTES)
            .build()
            .expect("the pool starts");
        let config = every_rule();
        let parsed = luck_parser::parse(source, version);
        let environments: &[StdlibEnvironment] = if version.is_luau() {
            &[StdlibEnvironment::Standalone, StdlibEnvironment::Roblox]
        } else {
            &[StdlibEnvironment::Standalone]
        };
        for &environment in environments {
            let diags = pool.install(|| lint_parsed(&parsed, version, environment, &config));
            let fixed = apply_fixes(source, &diags, version);
            let reparsed = luck_parser::parse(fixed.as_str(), version);
            assert!(reparsed.errors.is_empty(), "{:?}", reparsed.errors.first());
        }
    });
}
