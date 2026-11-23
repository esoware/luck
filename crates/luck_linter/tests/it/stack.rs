//! Every rule walks the tree, and the driver runs them on rayon's threads, so
//! those run on the smallest stack too.

use luck_allocator::Allocator;
use luck_ast::MIN_STACK_BYTES;
use luck_linter::{FixKind, LintConfig};
use luck_testgen::{deep_and_long_sources, on_the_smallest_stack};
use luck_token::LuaTarget;
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
        let pool =
            ThreadPoolBuilder::new().stack_size(MIN_STACK_BYTES).build().expect("the pool starts");
        let config = every_rule();
        for target in LuaTarget::ALL.into_iter().filter(|target| target.version() == version) {
            pool.install(|| {
                let allocator = Allocator::new();
                let fixed = luck_linter::fix(
                    &allocator,
                    source.as_bytes(),
                    target,
                    &config,
                    FixKind::all(),
                )
                .expect("the fixes converge");
                assert!(fixed.parse.errors.is_empty(), "{:?}", fixed.parse.errors.first());
            });
        }
    });
}
