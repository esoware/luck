use luck_value::MayHaveSideEffects as _;

use crate::common::{Locals, of_expr};

/// `x`, `y`, and `t` are locals, and every other name a global.
fn assert_effects(cases: &[(&str, bool)]) {
    let context = Locals { locals: &["x", "y", "t"] };
    for &(source, expected) in cases {
        of_expr(source, |expr| {
            assert_eq!(expr.may_have_side_effects(&context), expected, "{source}");
        });
    }
}

#[test]
fn a_global_read_may_run_index_and_a_local_read_runs_nothing() {
    assert_effects(&[
        ("x", false),
        ("g", true),
        ("...", false),
        ("function() return g end", false),
    ]);
}

#[test]
fn an_index_or_a_call_may_do_anything() {
    assert_effects(&[("t.k", true), ("t[1]", true), ("x()", true), ("x:m()", true)]);
}

#[test]
fn an_operator_runs_no_metamethod_on_values_whose_types_rule_one_out() {
    assert_effects(&[
        ("1 + 2", false),
        ("x + 1", true),
        ("-x", true),
        ("-'2'", true),
        ("'a' .. 1", false),
        ("'a' .. {}", true),
        ("#'abc'", false),
        ("#t", true),
        ("'a' < 'b'", false),
        ("1 < 'a'", true),
        ("not x", false),
        ("x and y", false),
        ("x and g", true),
        ("false and g()", false),
        ("x == nil", false),
        ("x ~= 1", false),
        ("x == {}", true),
        ("x == y", true),
    ]);
}

#[test]
fn a_number_divided_by_zero_raises_nothing() {
    assert_effects(&[("1 // 0", false), ("1 % 0", false), ("1 % 0.0", false), ("x % 0", true)]);
}

#[test]
fn a_table_constructor_raises_on_a_nil_or_nan_key() {
    assert_effects(&[
        ("{x, k = y}", false),
        ("{[1] = x, ['k'] = 2, [true] = 3}", false),
        ("{[x] = 1}", true),
        ("{[nil] = 1}", true),
        ("{[0/0] = 1}", true),
        ("{[0.3 ^ 3] = 1}", true),
        ("{[{}] = 1}", false),
        ("{g}", true),
    ]);
}

#[test]
fn types_and_text_alone_run_nothing() {
    assert_effects(&[
        ("(x :: number)", false),
        ("x<<number>>", false),
        ("`text`", false),
        ("`a{x}`", true),
        ("if x then y else 1", false),
        ("if x then g else 1", true),
        ("if true then 1 else g", false),
    ]);
}
