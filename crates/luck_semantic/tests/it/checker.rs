use std::ops::Range;

use luck_allocator::Allocator;
use luck_diagnostics::Diagnostic;
use luck_semantic::SemanticBuilder;

use crate::common::for_each_valid_source;

/// Parse errors, or the checker's after a clean parse, as `luck check`
/// reports them.
fn check(source: &str) -> Vec<Diagnostic> {
    let allocator = Allocator::new();
    let result = luck_parser::parse(&allocator, source.as_bytes());
    if !result.errors.is_empty() {
        return result.errors;
    }
    let chunk = allocator.alloc(result.chunk);
    SemanticBuilder::new().with_counts(result.counts).with_check(true).build(chunk).errors
}

fn assert_no_errors(errors: &[Diagnostic]) {
    assert!(errors.is_empty(), "unexpected: {errors:?}");
}

fn assert_has_errors(errors: &[Diagnostic], description: &str) {
    assert!(!errors.is_empty(), "expected errors for {description}, got none");
}

#[test]
fn assignment_to_const_rejected() {
    assert_has_errors(
        &check(
            "\
const x = 1
x = 2",
        ),
        "const reassignment",
    );
    assert_has_errors(
        &check(
            "\
const x = 1
x += 1",
        ),
        "const compound assignment",
    );
    assert_has_errors(
        &check(
            "\
const function f() end
f = nil",
        ),
        "const function reassignment",
    );
    // Upvalue writes are errors too.
    assert_has_errors(
        &check(
            "\
const x = 1
local f = function() x = 2 end",
        ),
        "const upvalue write",
    );
    // Reads, shadows, and table mutation stay legal.
    assert_no_errors(&check(
        "\
const x = 1
print(x)",
    ));
    assert_no_errors(&check(
        "\
const x = 1
local x = 2
x = 3
print(x)",
    ));
    assert_no_errors(&check(
        "\
const t = {}
t.x = 1
t[1] = 2",
    ));
}

#[test]
fn const_bindings_restore_after_nested_scopes() {
    for scoped in [
        "do local value = 0 value = 1 end",
        "while false do local value = 0 value = 1 end",
        "repeat local value = 0 value = 1 until true",
        "if a then local value = 0 value = 1 elseif b then local value = 0 value = 1 else local value = 0 value = 1 end",
        "for value = 1, 2 do print(value) end",
        "for value in pairs({}) do print(value) end",
        "local function f(value) value = 1 end",
        "local f = function(value) value = 1 end",
    ] {
        let source = format!(
            "\
const value = 1
{scoped}
value = 2"
        );
        let errors = check(&source);
        assert_eq!(errors.len(), 1, "{source}: {errors:?}");
        assert_eq!(errors[0].message, "attempt to assign to const variable `value`");
        assert_eq!(
            Range::<usize>::from(errors[0].span.unwrap()).start,
            source.rfind("value = 2").expect("final assignment")
        );
    }
    assert_no_errors(&check(
        "\
local value = 0
do const value = 1 end
value = 2",
    ));
}

#[test]
fn errors_come_in_source_order() {
    // Found in the walk's order, a value's writes before its own
    // assignment's, and reported in the source's.
    let source = "\
const x = 1
x = function() x = 2 end";
    let errors = check(source);
    let found: Vec<(usize, &str)> =
        errors.iter().map(|e| (Range::<usize>::from(e.span.unwrap()).start, &*e.message)).collect();
    let outer_write = source.find("x = function").expect("outer write");
    let inner_write = source.find("x = 2").expect("inner write");
    assert_eq!(
        found,
        [
            (outer_write, "attempt to assign to const variable `x`"),
            (inner_write, "attempt to assign to const variable `x`"),
        ]
    );
}

#[test]
fn loop_variables_are_writable() {
    assert_no_errors(&check("for i = 1, 3 do i = 5 end"));
    assert_no_errors(&check("for k, v in pairs(t) do k = 1 v = 2 end"));
}

#[test]
fn continue_until_local_capture_rejected() {
    assert_has_errors(
        &check("repeat do continue end local found = f() until found"),
        "until reads local declared after continue",
    );
    assert_no_errors(&check("repeat local found = f() do continue end until found"));
    assert_no_errors(&check("repeat do continue end local found = f() until other"));
}

#[test]
fn continue_until_reports_each_skipped_local_once() {
    let errors = check("repeat if cond then continue end local found = true until found and found");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].message.contains("`found`"), "{errors:?}");
}

#[test]
fn continue_until_reports_every_distinct_skipped_local() {
    let errors =
        check("repeat if cond then continue end local a = 1 local b = 2 until a and b and a");
    assert_eq!(errors.len(), 2, "{errors:?}");
}

#[test]
fn until_condition_resolves_const_writes_in_the_repeat_body_scope() {
    // The condition sees the body's locals, for writes as for reads.
    let shadowing_local = "const x = 1 repeat local x = 2 until (function() x = 3 end)()";
    let shadowing_errors = check(shadowing_local);
    assert!(
        shadowing_errors.is_empty(),
        "the write targets the body's plain local: {shadowing_errors:?}"
    );

    let body_const = "repeat const x = 2 until (function() x = 3 end)()";
    let body_const_errors = check(body_const);
    assert!(!body_const_errors.is_empty(), "the write targets the body's const local");
}

#[test]
fn method_self_shadows_a_const_local() {
    // `function t:m()` declares an implicit `self` parameter.
    let errors = check(
        "\
const self = 1
function t:m() self = 2 end",
    );
    assert!(errors.is_empty(), "the write targets the implicit parameter: {errors:?}");
}

#[test]
fn continue_until_ignores_names_rebound_inside_a_function() {
    for condition in ["(function(x) return x end)(1)", "(function() local x = 1 return x end)()"] {
        let source = format!("repeat if a then continue end local x = f() until {condition}");
        let errors = check(&source);
        assert!(errors.is_empty(), "{source} never reads the skipped local: {errors:?}");
    }
}

#[test]
fn continue_until_ignores_typeof_in_the_condition() {
    // Types generate no code, so `typeof(x)` does not read the skipped local.
    let source = "repeat if a then continue end local x = f() until (y :: typeof(x)) == 1";
    let errors = check(source);
    assert!(errors.is_empty(), "luau compiles this program: {errors:?}");
}

#[test]
fn type_function_cannot_reference_an_outer_local() {
    for source in [
        "\
local x = 1
type function f() return x end",
        "\
local x = 1
type function f() local function g() return x end return g end",
        "\
local function g() end
type function f() return g() end",
    ] {
        let errors = check(source);
        let [error] = &*errors else {
            panic!("luau rejects the outer local in {source}: {errors:?}");
        };
        assert!(error.message.starts_with("type function cannot reference outer local"));
    }
    let errors = check(
        "\
type function f(t)
    local inner = t
    return function() return inner, types.number end
end",
    );
    assert!(errors.is_empty(), "a type function's own locals are its to read: {errors:?}");
}

#[test]
fn valid_sources_check_clean() {
    for_each_valid_source(|source| {
        let errors = check(source);
        assert!(errors.is_empty(), "{errors:?}\n{source}");
    });
}

#[test]
fn a_const_binding_is_read_only_where_it_holds() {
    for source in [
        "if const x = f() then x = 2 end",
        "if a then elseif const x = f() then x += 1 end",
        "while const x = f() do x = nil end",
        "local g = if const x = f() then function() x = 1 end else nil",
    ] {
        assert_has_errors(&check(source), source);
    }
    for source in [
        "if local x = f() then x = 2 end",
        "while local x = f() do x = nil end",
        "local x = 0 if const x = f() then print(x) end x = 1",
    ] {
        assert_no_errors(&check(source));
    }
}
