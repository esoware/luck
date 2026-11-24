use luck_testgen::{generate_full_grammar, generate_runtime_safe, lua_fixtures};
use luck_token::LuaVersion;

/// Runs `check` on every fixture that isn't an error fixture, as the
/// version its path names, and on generated programs of every version.
pub(crate) fn for_each_valid_source(mut check: impl FnMut(&str, LuaVersion)) {
    let mut checked = 0usize;
    for fixture in lua_fixtures() {
        if fixture.is_error {
            continue;
        }
        check(&fixture.source, fixture.version);
        checked += 1;
    }
    assert!(checked > 0, "no fixture outside `errors/`");
    for version in LuaVersion::ALL {
        for seed in 0..40 {
            check(&generate_full_grammar(seed, version, 25), version);
            check(&generate_runtime_safe(seed, version, 25), version);
        }
    }
}
