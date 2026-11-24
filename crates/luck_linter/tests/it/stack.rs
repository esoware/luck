use luck_allocator::Allocator;
use luck_linter::{FixKind, LintConfig, Linter};
use luck_stdlib::library_for;
use luck_testgen::{deep_and_long_sources, on_the_smallest_stack};
use luck_token::LuaTarget;

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
        let linter = Linter::new(every_rule());
        for target in LuaTarget::ALL.into_iter().filter(|target| target.version() == version) {
            let allocator = Allocator::new();
            let fixed = linter
                .fix(&allocator, source.as_bytes(), library_for(target), FixKind::all())
                .expect("the fixes converge");
            assert!(fixed.parse.errors.is_empty(), "{:?}", fixed.parse.errors.first());
        }
    });
}
