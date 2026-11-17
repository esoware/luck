//! Every rule walks the tree, and the driver runs them on rayon's threads, so
//! those run on the smallest stack too.

use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_linter::{LintConfig, apply_fixes, lint_parsed};
use luck_testgen::{SMALLEST_STACK_BYTES, deep_and_long_sources, on_the_smallest_stack};
use luck_token::LuaVersion;
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
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source, version);
        for &target in targets(version) {
            let diags = pool.install(|| lint_parsed(&parsed, target, &config));
            if let Some(fixed) = apply_fixes(&allocator, source, &diags, version) {
                assert!(fixed.errors.is_empty(), "{:?}", fixed.errors.first());
            }
        }
    });
}

const fn targets(version: LuaVersion) -> &'static [LuaTarget] {
    match version {
        LuaVersion::Lua51 => &[LuaTarget::Lua51],
        LuaVersion::Lua52 => &[LuaTarget::Lua52],
        LuaVersion::Lua53 => &[LuaTarget::Lua53],
        LuaVersion::Lua54 => &[LuaTarget::Lua54],
        LuaVersion::Lua55 => &[LuaTarget::Lua55],
        LuaVersion::Luau => &[LuaTarget::Luau, LuaTarget::LuauRoblox],
    }
}
