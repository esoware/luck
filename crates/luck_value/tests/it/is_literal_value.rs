use luck_token::LuaVersion;
use luck_value::IsLiteralValue as _;

use crate::common::{Locals, of_expr};

fn assert_literal(version: LuaVersion, cases: &[(&str, bool)]) {
    let context = Locals { version, locals: &["x"] };
    for &(source, expected) in cases {
        of_expr(source, version, |expr| {
            assert_eq!(expr.is_literal_value(&context), expected, "{source} on {version:?}");
        });
    }
}

#[test]
fn a_literal_value_reads_nothing_and_builds_nothing() {
    assert_literal(
        LuaVersion::Lua54,
        &[
            ("1 + 2", true),
            ("0.3 ^ 3", true),
            ("'a' .. 1", true),
            ("not nil", true),
            ("1 // 0", false),
            ("x", false),
            ("x == nil", false),
            ("...", false),
            ("{}", false),
            ("function() end", false),
            ("f()", false),
        ],
    );
    assert_literal(LuaVersion::Luau, &[("if true then 1 else 2", true), ("`text`", true)]);
}

#[test]
fn an_operand_that_never_runs_still_counts() {
    assert_literal(
        LuaVersion::Lua54,
        &[("nil and x", false), ("1 or {}", false), ("false and 2", true)],
    );
    assert_literal(LuaVersion::Luau, &[("if true then 1 else x", false)]);
}
