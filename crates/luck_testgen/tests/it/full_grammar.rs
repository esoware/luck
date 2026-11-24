//! Regressions for `generate_full_grammar`: each pinned program breaks the
//! promise to parse under the official compiler or to reach the whole grammar.

use luck_testgen::generate_full_grammar;
use luck_token::LuaVersion;

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

fn indentation(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

#[test]
fn lua55_assigns_no_name_the_const_wildcard_makes_read_only() {
    let program = generate_full_grammar(42, LuaVersion::Lua55, 30);
    let lines: Vec<&str> = program.lines().collect();
    let wildcard = lines.iter().position(|line| line.trim_start() == "global <const> *");
    let Some(wildcard) = wildcard else {
        panic!("the full-grammar profile promises Lua 5.5's `global <const> *`\n{program}");
    };
    let depth = indentation(lines[wildcard]);
    let mut declared: Vec<&str> =
        lines[..wildcard].iter().copied().flat_map(module_declarations).collect();
    // The wildcard holds to the end of its block, which the first line
    // indented less than it closes.
    let scope = lines[wildcard + 1..]
        .iter()
        .take_while(|line| line.is_empty() || indentation(line) >= depth)
        .filter(|line| indentation(line) == depth);
    for line in scope {
        let statement = &line[depth..];
        declared.extend(module_declarations(statement));
        let target = statement.split_once(" = ").map(|(target, _)| target);
        let Some(name) =
            target.filter(|text| text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        else {
            continue;
        };
        assert!(
            declared.contains(&name),
            "`{statement}` assigns `{name}`, which nothing in scope declares, so it is a global \
             made read-only by `global <const> *` and Lua 5.5 rejects the chunk with `attempt \
             to assign to const variable`\n{program}"
        );
    }
}

#[test]
fn lua54_reaches_the_close_attribute() {
    let program = generate_full_grammar(42, LuaVersion::Lua54, 200);
    assert!(
        program.contains("<close>"),
        "the full-grammar profile promises the whole grammar and attributes, so a 5.4 program \
         must reach `<close>`, not only `<const>`\n{program}"
    );
}
