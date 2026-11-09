use core::ops::Range;

use luck_parser::ParseError;
use luck_token::LuaVersion;

/// Parse errors, or validation errors after a clean parse, as `luck check`
/// reports them.
fn check(source: &str, version: LuaVersion) -> Vec<ParseError> {
    let result = luck_parser::parse(source, version);
    if result.errors.is_empty() {
        luck_parser::validate(&result.block, version)
    } else {
        result.errors
    }
}

fn assert_no_errors(errors: &[ParseError]) {
    assert!(errors.is_empty(), "unexpected: {errors:?}");
}

fn assert_has_errors(errors: &[ParseError], description: &str) {
    assert!(!errors.is_empty(), "expected errors for {description}, got none");
}

#[test]
fn assignment_to_const_attrib_rejected() {
    for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_has_errors(&check("local x <const> = 1\nx = 2", version), "const reassignment");
        assert_has_errors(&check("local x <close> = nil\nx = 2", version), "close reassignment");
        // Upvalue writes are compile errors too.
        assert_has_errors(
            &check("local x <const> = 1\nlocal f = function() x = 2 end", version),
            "const upvalue write",
        );
        // Reads, shadows, and table mutation stay legal.
        assert_no_errors(&check("local x <const> = 1\nprint(x)", version));
        assert_no_errors(&check("local x <const> = 1\nlocal x = 2\nx = 3\nprint(x)", version));
        assert_no_errors(&check("local t <const> = {}\nt.x = 1\nt[1] = 2", version));
    }
}

#[test]
fn assignment_to_luau_const_rejected() {
    assert_has_errors(&check("const x = 1\nx = 2", LuaVersion::Luau), "luau const reassignment");
    assert_has_errors(
        &check("const x = 1\nx += 1", LuaVersion::Luau),
        "luau const compound assignment",
    );
    assert_has_errors(
        &check("const function f() end\nf = nil", LuaVersion::Luau),
        "luau const function reassignment",
    );
    assert_no_errors(&check("const x = 1\nprint(x)", LuaVersion::Luau));
}

#[test]
fn const_bindings_restore_after_nested_scopes() {
    for version in [LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        let declaration =
            if version.is_luau() { "const value = 1" } else { "local value <const> = 1" };
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
            let source = format!("{declaration}\n{scoped}\nvalue = 2");
            let errors = check(&source, version);
            assert_eq!(errors.len(), 1, "{version:?} {source}: {errors:?}");
            assert_eq!(errors[0].message, "attempt to assign to const variable `value`");
            assert_eq!(
                Range::<usize>::from(errors[0].span).start,
                source.rfind("value = 2").expect("final assignment")
            );
        }
        let source = format!("local value = 0\ndo {declaration} end\nvalue = 2");
        assert_no_errors(&check(&source, version));
    }
    // A named vararg shadows the const, but it is read-only itself.
    assert_has_errors(
        &check(
            "local value <const> = 1\nlocal function f(...value) value = 2 end",
            LuaVersion::Lua55,
        ),
        "write to a named vararg",
    );
}

#[test]
fn validation_errors_follow_check_and_walk_order() {
    // Not sorted by position: const writes come before goto errors, a value's
    // writes before its own assignment's, and function bodies' gotos before
    // the main chunk's.
    let source = "goto top
local x <const> = 1
x = function() x = 2 goto inner end";
    let errors = check(source, LuaVersion::Lua54);
    let found: Vec<(usize, &str)> =
        errors.iter().map(|e| (Range::<usize>::from(e.span).start, e.message.as_str())).collect();
    let outer_write = source.find("x = function").expect("outer write");
    let inner_write = source.find("x = 2").expect("inner write");
    let inner_goto = source.find("goto inner").expect("inner goto");
    assert_eq!(
        found,
        [
            (inner_write, "attempt to assign to const variable `x`"),
            (outer_write, "attempt to assign to const variable `x`"),
            (inner_goto, "no visible label `inner` for `goto`"),
            (0, "no visible label `top` for `goto`"),
        ]
    );
}

#[test]
fn lua55_for_variables_are_readonly() {
    assert_has_errors(
        &check("for i = 1, 3 do i = 5 end", LuaVersion::Lua55),
        "5.5 numeric for var write",
    );
    assert_has_errors(
        &check("for k, v in pairs(t) do k = 1 end", LuaVersion::Lua55),
        "5.5 generic for control var write",
    );
    // Valid before 5.5.
    assert_no_errors(&check("for i = 1, 3 do i = 5 end", LuaVersion::Lua54));
    assert_no_errors(&check("for i = 1, 3 do i = 5 end", LuaVersion::Lua51));
}

#[test]
fn goto_undefined_label_rejected() {
    assert_has_errors(&check("do goto nowhere end", LuaVersion::Lua54), "undefined label");
    assert_has_errors(
        &check(
            "goto onlyinfunction\nlocal f = function() ::onlyinfunction:: end",
            LuaVersion::Lua54,
        ),
        "label not visible across functions",
    );
    assert_no_errors(&check("::top:: goto top", LuaVersion::Lua54));
    assert_no_errors(&check("do goto out end ::out::", LuaVersion::Lua54));
}

#[test]
fn duplicate_label_rejected() {
    assert_has_errors(&check("::l:: ::l::", LuaVersion::Lua54), "duplicate label in block");
    assert_has_errors(
        &check("::l:: do ::l:: goto l end", LuaVersion::Lua54),
        "shadowing visible label",
    );
    // Same label name in sibling functions is fine.
    assert_no_errors(&check(
        "local f = function() ::l:: goto l end\nlocal g = function() ::l:: goto l end",
        LuaVersion::Lua54,
    ));
}

#[test]
fn goto_into_local_scope_rejected() {
    assert_has_errors(
        &check("do goto skip end local x = 1 ::skip:: print(x)", LuaVersion::Lua54),
        "jump into local scope",
    );
    // A label at the end of the block escapes the locals' scope.
    assert_no_errors(&check("do goto skip end local x = 1 ::skip::", LuaVersion::Lua54));
    // The goto-continue loop idiom stays valid.
    assert_no_errors(&check(
        "for i = 1, 3 do if i == 2 then goto continue end local y = i ::continue:: end",
        LuaVersion::Lua54,
    ));
    // In a repeat block, `until` sees the locals, so a trailing label
    // doesn't escape their scope.
    assert_has_errors(
        &check(
            "repeat if a then goto continue end local x = 1 ::continue:: until x",
            LuaVersion::Lua54,
        ),
        "jump over local visible to until",
    );
}

#[test]
fn goto_resolves_against_enclosing_blocks_when_it_leaves_them() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_no_errors(&check("::l:: do do goto l end end", version));
        assert_no_errors(&check("::l:: local x do goto l end", version));
    }
    // The inner `::l::` takes the goto first, before 5.4 banned shadowing.
    for version in [LuaVersion::Lua52, LuaVersion::Lua53] {
        let errors = check("::l:: do goto l local x ::l:: print(x) end", version);
        assert_eq!(errors.len(), 1, "{version:?}: {errors:?}");
        assert_eq!(errors[0].message, "`goto l` jumps into the scope of a local");
    }
}

#[test]
fn goto_trailing_labels_and_empty_stmts() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for source in [
            "",
            "; ::first:: ; ::second:: ;",
            "goto skip local value = 1 ::skip:: ; ::last:: ;",
            "do goto skip end local value = 1 ::skip:: ; ::last:: ;",
        ] {
            assert_no_errors(&check(source, version));
        }
        for source in [
            "goto skip local value = 1 ::skip:: ; print(value) ::last:: ;",
            "repeat goto skip local value = 1 ::skip:: ; ::last:: ; until value",
        ] {
            let errors = check(source, version);
            assert_eq!(errors.len(), 1, "{version:?} {source}: {errors:?}");
            assert_eq!(errors[0].message, "`goto skip` jumps into the scope of a local");
        }
    }
}

#[test]
fn continue_until_local_capture_rejected() {
    assert_has_errors(
        &check("repeat do continue end local found = f() until found", LuaVersion::Luau),
        "until reads local declared after continue",
    );
    assert_no_errors(&check(
        "repeat local found = f() do continue end until found",
        LuaVersion::Luau,
    ));
    assert_no_errors(&check(
        "repeat do continue end local found = f() until other",
        LuaVersion::Luau,
    ));
}

#[test]
fn continue_until_reports_each_skipped_local_once() {
    let errors = check(
        "repeat if cond then continue end local found = true until found and found",
        LuaVersion::Luau,
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].message.contains("`found`"), "{errors:?}");
}

#[test]
fn continue_until_reports_every_distinct_skipped_local() {
    let errors = check(
        "repeat if cond then continue end local a = 1 local b = 2 until a and b and a",
        LuaVersion::Luau,
    );
    assert_eq!(errors.len(), 2, "{errors:?}");
}

#[test]
fn goto_cannot_skip_a_local_to_reach_a_label_before_return() {
    // A label followed by `return` isn't at the end of its block.
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let errors = check("goto l local x ::l:: return", version);
        assert!(
            errors.iter().any(|e| e.message.contains("jumps into the scope")),
            "{version:?} must reject the jump into the scope of `x`: {errors:?}"
        );
    }
}

#[test]
fn lua52_and_lua53_allow_a_label_to_shadow_an_enclosing_label() {
    // `checkrepeated` only looks at the current block before 5.4.
    for version in [LuaVersion::Lua52, LuaVersion::Lua53] {
        let errors = check("::l:: do ::l:: end", version);
        assert!(errors.is_empty(), "{version:?} accepts this program: {errors:?}");
    }
}

#[test]
fn lua55_only_the_first_generic_for_variable_is_readonly() {
    // 5.5 manual section 3.3.5: the first variable is the control variable.
    for source in ["for k, v in pairs(t) do v = 1 end", "for k, v, w in pairs(t) do w = 1 end"] {
        let errors = check(source, LuaVersion::Lua55);
        assert!(errors.is_empty(), "{source} is valid in Lua 5.5: {errors:?}");
    }
}

#[test]
fn lua55_flags_undeclared_name_after_global_declaration() {
    // A global declaration voids the implicit `global *`.
    let errors = check("global x; y = 1", LuaVersion::Lua55);
    assert!(!errors.is_empty(), "`y` is not declared, so Lua 5.5 rejects the write");
}

#[test]
fn lua55_env_stays_visible_under_a_global_declaration() {
    let errors = check("global x; local e = _ENV", LuaVersion::Lua55);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn lua55_const_global_star_makes_undeclared_names_readonly() {
    for source in ["global <const> *; y = 1", "global <const> *; function y() end"] {
        let errors = check(source, LuaVersion::Lua55);
        assert!(
            errors.iter().any(|e| e.message.contains("const variable `y`")),
            "{source}: `y` is read-only under `global <const> *`: {errors:?}"
        );
    }
}

#[test]
fn lua55_global_function_shadows_a_const_binding() {
    for source in [
        "local x <const> = 1; global function x() end; x = 2",
        "global <const> x; global function x() end; x = 2",
    ] {
        let errors = check(source, LuaVersion::Lua55);
        assert!(errors.is_empty(), "{source} is valid in Lua 5.5: {errors:?}");
    }
}

#[test]
fn lua55_goto_cannot_enter_the_scope_of_a_global_declaration() {
    // A global declaration occupies a slot in `nactvar`, like a local.
    for source in [
        "goto l; global x; ::l:: x = 1",
        "goto l; global *; ::l:: print(1)",
        "goto l; global function f() end; ::l:: f()",
    ] {
        let errors = check(source, LuaVersion::Lua55);
        assert!(
            errors.iter().any(|e| e.message.contains("jumps into the scope")),
            "{source} must be rejected as a jump into a declaration's scope: {errors:?}"
        );
    }
}

#[test]
fn lua55_named_vararg_is_readonly() {
    // 5.5 manual: the name denotes a read-only local variable.
    let errors = check("function f(...t) t = 1 end", LuaVersion::Lua55);
    assert!(
        errors.iter().any(|e| e.message.contains("const variable `t`")),
        "writing a named vararg must be rejected: {errors:?}"
    );
}

#[test]
fn until_condition_resolves_const_writes_in_the_repeat_body_scope() {
    // The condition sees the body's locals, for writes as for reads.
    let shadowing_local = "local x <const> = 1 repeat local x = 2 until (function() x = 3 end)()";
    let shadowing_errors = check(shadowing_local, LuaVersion::Lua54);
    assert!(
        shadowing_errors.is_empty(),
        "the write targets the body's plain local: {shadowing_errors:?}"
    );

    let body_const = "repeat local x <const> = 2 until (function() x = 3 end)()";
    let body_const_errors = check(body_const, LuaVersion::Lua54);
    assert!(!body_const_errors.is_empty(), "the write targets the body's const local");
}

#[test]
fn method_self_shadows_a_const_local() {
    // `function t:m()` declares an implicit `self` parameter.
    let errors = check("local self <const> = 1\nfunction t:m() self = 2 end", LuaVersion::Lua54);
    assert!(errors.is_empty(), "the write targets the implicit parameter: {errors:?}");
}

#[test]
fn continue_until_ignores_names_rebound_inside_a_function() {
    for condition in ["(function(x) return x end)(1)", "(function() local x = 1 return x end)()"] {
        let source = format!("repeat if a then continue end local x = f() until {condition}");
        let errors = check(&source, LuaVersion::Luau);
        assert!(errors.is_empty(), "{source} never reads the skipped local: {errors:?}");
    }
}

#[test]
fn continue_until_ignores_typeof_in_the_condition() {
    // Types generate no code, so `typeof(x)` does not read the skipped local.
    let source = "repeat if a then continue end local x = f() until (y :: typeof(x)) == 1";
    let errors = check(source, LuaVersion::Luau);
    assert!(errors.is_empty(), "luau compiles this program: {errors:?}");
}
