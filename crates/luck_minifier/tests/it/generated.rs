//! Generated programs and the shared fixtures, each of which has to minify to
//! output that reparses and minifies to itself.

use luck_minifier::TransformConfig;
use luck_testgen::{generate_full_grammar, generate_runtime_safe, lua_fixtures};
use luck_token::LuaVersion;

use crate::common::minify_with;

const SEED_COUNT: u64 = 60;
const PER_PASS_SEED_COUNT: u64 = 15;
const STATEMENT_BUDGET: usize = 25;

#[test]
fn generated_programs_minify_to_a_fixpoint() {
    let config = TransformConfig::default();
    for version in LuaVersion::ALL {
        for seed in 0..SEED_COUNT {
            minify_with(&generate_runtime_safe(seed, version, STATEMENT_BUDGET), version, &config);
            minify_with(&generate_full_grammar(seed, version, STATEMENT_BUDGET), version, &config);
        }
    }
}

#[test]
fn generated_programs_minify_to_a_fixpoint_with_each_pass_alone() {
    let none = TransformConfig {
        strip_types: false,
        remove_dead_code: false,
        simplify_statements: false,
        fold_constants: false,
        inline_locals: false,
        merge_locals: false,
        simplify_indexes: false,
        simplify_parens: false,
        rename_locals: false,
        lift_locals: false,
        rename_globals: false,
    };
    let configs = [
        TransformConfig { strip_types: true, ..none },
        TransformConfig { remove_dead_code: true, ..none },
        TransformConfig { simplify_statements: true, ..none },
        TransformConfig { fold_constants: true, ..none },
        TransformConfig { inline_locals: true, ..none },
        TransformConfig { merge_locals: true, ..none },
        TransformConfig { simplify_indexes: true, ..none },
        TransformConfig { simplify_parens: true, ..none },
        TransformConfig { rename_locals: true, ..none },
        TransformConfig { lift_locals: true, ..none },
        TransformConfig { rename_globals: true, ..TransformConfig::default() },
    ];
    for config in configs {
        for version in LuaVersion::ALL {
            for seed in 0..PER_PASS_SEED_COUNT {
                minify_with(
                    &generate_full_grammar(seed, version, STATEMENT_BUDGET),
                    version,
                    &config,
                );
            }
        }
    }
}

#[test]
fn fixtures_minify_to_a_fixpoint() {
    let config = TransformConfig::default();
    for fixture in lua_fixtures() {
        if fixture.is_error {
            continue;
        }
        minify_with(&fixture.source.replace("\r\n", "\n"), fixture.version, &config);
    }
}
