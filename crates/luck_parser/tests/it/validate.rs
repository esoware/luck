//! Scope-context validation: const writes, goto/label resolution, and
//! Luau's continue/until rule (all compile errors in real Lua).

use luck_token::LuaVersion;

/// Parse and run the opt-in scope-context validation, folding its
/// errors into the result (mirrors what `luck check` does).
struct Validated {
    errors: Vec<luck_parser::ParseError>,
}

fn parse(source: &str, version: LuaVersion) -> Validated {
    let result = luck_parser::parse(source, version);
    let mut errors = result.errors;
    if errors.is_empty() {
        errors = luck_parser::validate(&result.block, version);
    }
    Validated { errors }
}

fn assert_no_errors(result: &Validated) {
    assert!(result.errors.is_empty(), "unexpected: {:?}", result.errors);
}

fn assert_has_errors(result: &Validated, what: &str) {
    assert!(
        !result.errors.is_empty(),
        "expected errors for {what}, got none"
    );
}

#[test]
fn assignment_to_const_attrib_rejected() {
    for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_has_errors(
            &parse("local x <const> = 1\nx = 2", version),
            "const reassignment",
        );
        assert_has_errors(
            &parse("local x <close> = nil\nx = 2", version),
            "close reassignment",
        );
        // Upvalue writes are compile errors too.
        assert_has_errors(
            &parse(
                "local x <const> = 1\nlocal f = function() x = 2 end",
                version,
            ),
            "const upvalue write",
        );
        // Reads, shadows, and table mutation stay legal.
        assert_no_errors(&parse("local x <const> = 1\nprint(x)", version));
        assert_no_errors(&parse(
            "local x <const> = 1\nlocal x = 2\nx = 3\nprint(x)",
            version,
        ));
        assert_no_errors(&parse("local t <const> = {}\nt.x = 1\nt[1] = 2", version));
    }
}

#[test]
fn assignment_to_luau_const_rejected() {
    assert_has_errors(
        &parse("const x = 1\nx = 2", LuaVersion::Luau),
        "luau const reassignment",
    );
    assert_has_errors(
        &parse("const x = 1\nx += 1", LuaVersion::Luau),
        "luau const compound assignment",
    );
    assert_has_errors(
        &parse("const function f() end\nf = nil", LuaVersion::Luau),
        "luau const function reassignment",
    );
    assert_no_errors(&parse("const x = 1\nprint(x)", LuaVersion::Luau));
}

#[test]
fn const_bindings_restore_after_nested_scopes() {
    for version in [LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        let declaration = if version.is_luau() {
            "const value = 1"
        } else {
            "local value <const> = 1"
        };
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
            let result = parse(&source, version);
            assert_eq!(
                result.errors.len(),
                1,
                "{version:?} {source}: {:?}",
                result.errors
            );
            assert_eq!(
                result.errors[0].message,
                "attempt to assign to const variable 'value'"
            );
            assert_eq!(
                result.errors[0].span.start as usize,
                source.rfind("value = 2").expect("final assignment")
            );
        }
        let source = format!("local value = 0\ndo {declaration} end\nvalue = 2");
        assert_no_errors(&parse(&source, version));
    }
    assert_no_errors(&parse(
        "local value <const> = 1\nlocal function f(...value) value = 2 end",
        LuaVersion::Lua55,
    ));
}

#[test]
fn lua55_for_variables_are_readonly() {
    assert_has_errors(
        &parse("for i = 1, 3 do i = 5 end", LuaVersion::Lua55),
        "5.5 numeric for var write",
    );
    assert_has_errors(
        &parse("for k, v in pairs(t) do v = 1 end", LuaVersion::Lua55),
        "5.5 generic for var write",
    );
    // Valid before 5.5.
    assert_no_errors(&parse("for i = 1, 3 do i = 5 end", LuaVersion::Lua54));
    assert_no_errors(&parse("for i = 1, 3 do i = 5 end", LuaVersion::Lua51));
}

#[test]
fn goto_undefined_label_rejected() {
    assert_has_errors(
        &parse("do goto nowhere end", LuaVersion::Lua54),
        "undefined label",
    );
    assert_has_errors(
        &parse(
            "goto onlyinfunction\nlocal f = function() ::onlyinfunction:: end",
            LuaVersion::Lua54,
        ),
        "label not visible across functions",
    );
    assert_no_errors(&parse("::top:: goto top", LuaVersion::Lua54));
    assert_no_errors(&parse("do goto out end ::out::", LuaVersion::Lua54));
}

#[test]
fn duplicate_label_rejected() {
    assert_has_errors(
        &parse("::l:: ::l::", LuaVersion::Lua54),
        "duplicate label in block",
    );
    assert_has_errors(
        &parse("::l:: do ::l:: goto l end", LuaVersion::Lua54),
        "shadowing visible label",
    );
    // Same label name in sibling functions is fine.
    assert_no_errors(&parse(
        "local f = function() ::l:: goto l end\nlocal g = function() ::l:: goto l end",
        LuaVersion::Lua54,
    ));
}

#[test]
fn goto_into_local_scope_rejected() {
    assert_has_errors(
        &parse(
            "do goto skip end local x = 1 ::skip:: print(x)",
            LuaVersion::Lua54,
        ),
        "jump into local scope",
    );
    // A label at the end of the block escapes the locals' scope.
    assert_no_errors(&parse(
        "do goto skip end local x = 1 ::skip::",
        LuaVersion::Lua54,
    ));
    // The goto-continue loop idiom stays valid.
    assert_no_errors(&parse(
        "for i = 1, 3 do if i == 2 then goto continue end local y = i ::continue:: end",
        LuaVersion::Lua54,
    ));
    // In a repeat block, `until` sees the locals, so a trailing label
    // does NOT escape their scope.
    assert_has_errors(
        &parse(
            "repeat if a then goto continue end local x = 1 ::continue:: until x",
            LuaVersion::Lua54,
        ),
        "jump over local visible to until",
    );
}

#[test]
fn goto_trailing_labels_and_empty_statements() {
    for version in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        for source in [
            "",
            "; ::first:: ; ::second:: ;",
            "goto skip local value = 1 ::skip:: ; ::last:: ;",
            "do goto skip end local value = 1 ::skip:: ; ::last:: ;",
        ] {
            assert_no_errors(&parse(source, version));
        }
        for source in [
            "goto skip local value = 1 ::skip:: ; print(value) ::last:: ;",
            "repeat goto skip local value = 1 ::skip:: ; ::last:: ; until value",
        ] {
            let result = parse(source, version);
            assert_eq!(
                result.errors.len(),
                1,
                "{version:?} {source}: {:?}",
                result.errors
            );
            assert_eq!(
                result.errors[0].message,
                "goto 'skip' jumps into the scope of a local"
            );
        }
    }
}

#[test]
fn continue_until_local_capture_rejected() {
    assert_has_errors(
        &parse(
            "repeat do continue end local found = f() until found",
            LuaVersion::Luau,
        ),
        "until reads local declared after continue",
    );
    assert_no_errors(&parse(
        "repeat local found = f() do continue end until found",
        LuaVersion::Luau,
    ));
    assert_no_errors(&parse(
        "repeat do continue end local found = f() until other",
        LuaVersion::Luau,
    ));
}
