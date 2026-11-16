//! Analysis walks the whole tree, and the language server analyzes what's
//! typed as it's typed, broken or not.

use luck_testgen::{
    broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_past_the_nesting_cap,
};
use luck_token::StdlibEnvironment;

#[test]
fn analyzing_fits_the_smallest_stack() {
    let sources = deep_and_long_sources()
        .into_iter()
        .chain(sources_past_the_nesting_cap())
        .chain(broken_sources());
    on_the_smallest_stack(sources, |source, version| {
        let parsed = luck_parser::parse(source, version);
        let environments: &[StdlibEnvironment] = if version.is_luau() {
            &[StdlibEnvironment::Standalone, StdlibEnvironment::Roblox]
        } else {
            &[StdlibEnvironment::Standalone]
        };
        for &environment in environments {
            let analysis =
                luck_semantic::analyze_with_environment(&parsed.block, version, environment);
            assert!(!analysis.scope_tree.scopes.is_empty(), "the chunk has a scope");
        }
    });
}
