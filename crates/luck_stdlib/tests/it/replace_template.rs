use crate::common::expand;

#[test]
fn substitutes_numbered_args() {
    assert_eq!(expand("#%1", &["t"]), "#t");
    assert_eq!(expand("%1 ^ %2", &["a", "b"]), "a ^ b");
    assert_eq!(expand("load(%1)", &["x"]), "load(x)");
}

#[test]
fn unescapes_double_percent() {
    assert_eq!(expand("100%% %1", &["x"]), "100% x");
}

#[test]
fn expands_missing_arg_to_nothing() {
    assert_eq!(expand("%1 + %2", &["a"]), "a + ");
}

#[test]
fn groups_compound_operands() {
    assert_eq!(expand("(%1 ^ %2)", &["a + 1", "b"]), "((a + 1) ^ b)");
    assert_eq!(expand("#%1", &["a or b"]), "#(a or b)");
    assert_eq!(expand("#%1", &["..."]), "#(...)");
    assert_eq!(expand("#%1", &["a.b.c"]), "#a.b.c");
    assert_eq!(expand("(%1 ^ %2)", &["a", "-1"]), "(a ^ (-1))");
}

#[test]
fn keeps_call_arguments_verbatim() {
    assert_eq!(expand("math.log(%1, 10)", &["a + 1"]), "math.log(a + 1, 10)");
    assert_eq!(expand("string.gmatch(%1, %2)", &["s", "p .. q"]), "string.gmatch(s, p .. q)");
}

#[test]
fn forwards_trailing_arguments() {
    assert_eq!(expand("table.unpack(%1)", &["t", "2", "#t"]), "table.unpack(t, 2, #t)");
    assert_eq!(expand("task.wait(%1)", &[]), "task.wait()");
}
