use luck_allocator::Allocator;
use luck_linter::{FixKind, LintConfig, Linter};
use luck_stdlib::{SecurityLevel, luau_library, roblox_library};
use luck_testgen::{deep_and_long_sources, on_the_smallest_stack};

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
    on_the_smallest_stack(deep_and_long_sources(), |source| {
        let linter = Linter::new(every_rule());
        for library in [luau_library(), roblox_library(SecurityLevel::None)] {
            let allocator = Allocator::new();
            let parse = luck_parser::parse(&allocator, source.as_bytes());
            let fixed = linter
                .fix(&allocator, parse, library, FixKind::all(), None)
                .expect("the fixes converge");
            assert!(fixed.parse.errors.is_empty(), "{:?}", fixed.parse.errors.first());
        }
    });
}
