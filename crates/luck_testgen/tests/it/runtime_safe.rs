//! Regressions for `generate_runtime_safe`: each pinned program breaks the
//! parse-and-run guarantee on the stock interpreters.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use luck_testgen::generate_runtime_safe;
use luck_token::LuaVersion;

// Lua's own limit: `too many local variables (limit is 200)`.
const MAX_LOCALS_PER_FUNCTION: usize = 200;

fn tab_depth(line: &str) -> usize {
    line.chars().take_while(|character| *character == '\t').count()
}

/// The name and value of a final `print("name", value)` line.
fn final_print(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("print(\"")?;
    let (name, value) = rest.split_once("\", ")?;
    Some((name, value.strip_suffix(')')?))
}

/// The last line declaring `name` at module scope.
fn last_module_declaration<'program>(program: &'program str, name: &str) -> Option<&'program str> {
    let value_prefix = format!("local {name} = ");
    let function_prefix = format!("local function {name}(");
    program
        .lines()
        .rev()
        .find(|line| line.starts_with(&value_prefix) || line.starts_with(&function_prefix))
}

/// The depth at which `statement`, sitting at `depth`, starts hiding `name`.
fn shadow_depth(statement: &str, name: &str, depth: usize) -> Option<usize> {
    let is_local = statement.starts_with(&format!("local {name} = "))
        || statement.starts_with(&format!("local function {name}("));
    let is_block_binding = statement.starts_with(&format!("for {name} = "))
        || (statement.starts_with("local function ") && statement.ends_with(&format!("({name})")));
    if is_local {
        Some(depth)
    } else if is_block_binding {
        Some(depth.saturating_add(1))
    } else {
        None
    }
}

#[test]
fn bug_runtime_final_prints_take_the_length_of_tables_only() {
    let program = generate_runtime_safe(42, LuaVersion::Luau, 30);
    for line in program.lines() {
        let Some((name, value)) = final_print(line) else {
            continue;
        };
        if value != format!("#{name}") {
            continue;
        }
        let table_prefix = format!("local {name} = {{");
        let declaration = last_module_declaration(&program, name);
        assert!(
            declaration.is_some_and(|text| text.starts_with(&table_prefix)),
            "`{line}` must only take the length of a table, but `{name}` resolves to \
             {declaration:?}, so `#` raises a runtime error\n{program}"
        );
    }
}

#[test]
fn bug_runtime_print_statements_never_print_a_table() {
    let program = generate_runtime_safe(42, LuaVersion::Lua54, 30);
    let printed_table = program.lines().find(|line| line.trim_start().starts_with("print({"));
    assert_eq!(
        printed_table, None,
        "printing a table writes its heap address to stdout, which differs between the \
         original and transformed runs a differential test compares\n{program}"
    );
}

#[test]
fn bug_runtime_final_prints_never_report_a_local_function() {
    let program = generate_runtime_safe(42, LuaVersion::Lua55, 30);
    for line in program.lines() {
        let Some((name, value)) = final_print(line) else {
            continue;
        };
        if value != name {
            continue;
        }
        let declaration = last_module_declaration(&program, name);
        assert!(
            !declaration.is_some_and(|text| text.starts_with("local function ")),
            "`{line}` was tracked as a value, but `{name}` resolves to {declaration:?}: an \
             untracked `local function` must not take over a tracked binding's name\n{program}"
        );
    }
}

#[test]
fn bug_runtime_while_bodies_never_write_their_counter() {
    let program = generate_runtime_safe(7, LuaVersion::Lua54, 120);
    let lines: Vec<&str> = program.lines().collect();
    for (header_index, header) in lines.iter().enumerate() {
        let Some((counter, _)) =
            header.trim_start().strip_prefix("while ").and_then(|rest| rest.split_once(" < "))
        else {
            continue;
        };
        let body_depth = tab_depth(header).saturating_add(1);
        // The first body line is the loop's own increment.
        let body = lines
            .iter()
            .skip(header_index.saturating_add(2))
            .take_while(|line| tab_depth(line) >= body_depth);
        let mut hidden_from: Option<usize> = None;
        for line in body {
            let depth = tab_depth(line);
            if hidden_from.is_some_and(|hidden_depth| depth < hidden_depth) {
                hidden_from = None;
            }
            if hidden_from.is_some() {
                continue;
            }
            let statement = line.trim_start();
            hidden_from = shadow_depth(statement, counter, depth);
            assert!(
                !statement.starts_with(&format!("{counter} = "))
                    && !statement.starts_with(&format!("{counter} += ")),
                "`{statement}` writes the counter of `{}`, so the loop is no longer bounded\n\
                 {program}",
                header.trim_start()
            );
        }
    }
}

#[test]
fn bug_runtime_until_conditions_read_their_counter_not_a_body_local() {
    let program = generate_runtime_safe(7, LuaVersion::Lua54, 120);
    let lines: Vec<&str> = program.lines().collect();
    for (header_index, header) in lines.iter().enumerate() {
        if header.trim_start() != "repeat" {
            continue;
        }
        let header_depth = tab_depth(header);
        let body_depth = header_depth.saturating_add(1);
        let after_header = || lines.iter().skip(header_index.saturating_add(1));
        let until = after_header()
            .find(|line| tab_depth(line) == header_depth && line.trim_start().starts_with("until "))
            .expect("every generated repeat closes with until");
        let (counter, _) = until
            .trim_start()
            .strip_prefix("until ")
            .and_then(|rest| rest.split_once(" >= "))
            .expect("every generated until compares its counter");
        for line in after_header().take_while(|line| tab_depth(line) >= body_depth) {
            let is_body_level = tab_depth(line) == body_depth;
            let statement = line.trim_start();
            let hides_counter = statement.starts_with(&format!("local {counter} = "))
                || statement.starts_with(&format!("local function {counter}("));
            assert!(
                !(is_body_level && hides_counter),
                "`{statement}` hides the counter from `{}`, which can see the block's locals, \
                 so the comparison errors or never becomes true\n{program}",
                until.trim_start()
            );
        }
    }
}

#[test]
fn bug_runtime_function_returns_add_a_number_not_a_shadowing_string() {
    let program = generate_runtime_safe(7, LuaVersion::Lua54, 120);
    let lines: Vec<&str> = program.lines().collect();
    for (header_index, header) in lines.iter().enumerate() {
        let Some(param) = header
            .trim_start()
            .strip_prefix("local function ")
            .and_then(|rest| rest.split_once('('))
            .and_then(|(_, rest)| rest.strip_suffix(')'))
        else {
            continue;
        };
        let body_depth = tab_depth(header).saturating_add(1);
        let string_prefix = format!("local {param} = (\"");
        let body = lines
            .iter()
            .skip(header_index.saturating_add(1))
            .take_while(|line| tab_depth(line) >= body_depth);
        for line in body {
            let is_body_level = tab_depth(line) == body_depth;
            assert!(
                !(is_body_level && line.trim_start().starts_with(&string_prefix)),
                "`{}` rebinds parameter `{param}` to a string, and the function's closing \
                 `return {param} + ...` then raises a runtime error\n{program}",
                line.trim_start()
            );
        }
    }
}

#[test]
fn bug_runtime_module_scope_stays_within_the_local_limit() {
    let program = generate_runtime_safe(42, LuaVersion::Lua54, 500);
    let module_locals = program.lines().filter(|line| line.starts_with("local ")).count();
    assert!(
        module_locals <= MAX_LOCALS_PER_FUNCTION,
        "{module_locals} module-scope locals: PUC-Rio Lua rejects a function with more than \
         {MAX_LOCALS_PER_FUNCTION}, so the program must stay within the limit to parse"
    );
}
