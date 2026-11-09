//! Regressions for `generate_full`: each pinned program breaks the promise to
//! parse under the official compiler or to reach the whole grammar.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use luck_testgen::generate_full;
use luck_token::LuaVersion;

// Lua's own limit: `too many local variables (limit is 200)`.
const MAX_LOCALS_PER_FUNCTION: usize = 200;

/// The names a module-scope `local` or `global function` line declares.
fn module_declarations(line: &str) -> Vec<&str> {
    let Some(declaration) =
        line.strip_prefix("local ").or_else(|| line.strip_prefix("global function "))
    else {
        return Vec::new();
    };
    let without_function = declaration.strip_prefix("function ").unwrap_or(declaration);
    let without_attribute = without_function.strip_prefix("<const> ").unwrap_or(without_function);
    let name_list =
        without_attribute.split_once(['=', '(']).map_or(without_attribute, |(names, _)| names);
    name_list.split(',').map(|name| name.split_whitespace().next().unwrap_or_default()).collect()
}

#[test]
fn bug_full_lua55_assigns_no_name_the_const_wildcard_makes_read_only() {
    let program = generate_full(42, LuaVersion::Lua55, 30);
    if !program.lines().any(|line| line == "global <const> *") {
        return;
    }
    let mut declared: Vec<&str> = Vec::new();
    for line in program.lines() {
        declared.extend(module_declarations(line));
        let target = line.split_once(" = ").map(|(target, _)| target);
        let Some(name) = target.filter(|text| {
            text.chars().all(|character| character.is_ascii_alphanumeric() || character == '_')
        }) else {
            continue;
        };
        assert!(
            declared.contains(&name),
            "`{line}` assigns `{name}`, which no module-scope declaration binds, so it is a \
             global made read-only by `global <const> *` and Lua 5.5 rejects the chunk with \
             `attempt to assign to const variable`\n{program}"
        );
    }
}

#[test]
fn bug_full_module_scope_stays_within_the_local_limit() {
    let program = generate_full(42, LuaVersion::Lua54, 800);
    let module_locals = program.lines().filter(|line| line.starts_with("local ")).count();
    assert!(
        module_locals <= MAX_LOCALS_PER_FUNCTION,
        "{module_locals} module-scope locals: PUC-Rio Lua rejects a function with more than \
         {MAX_LOCALS_PER_FUNCTION}, so the program must stay within the limit to parse"
    );
}

#[test]
fn bug_full_lua54_reaches_the_close_attribute() {
    let program = generate_full(42, LuaVersion::Lua54, 200);
    assert!(
        program.contains("<close>"),
        "the full-grammar profile promises the whole grammar and attributes, so a 5.4 program \
         must reach `<close>`, not only `<const>`\n{program}"
    );
}
