//! Lua 5.1 to 5.5 reject a function with more active locals or upvalues than
//! their compilers count while they parse, and the checker reports each
//! where the compiler does.

use std::fmt::Write as _;

use luck_allocator::Allocator;
use luck_diagnostics::Diagnostic;
use luck_semantic::SemanticBuilder;
use luck_token::{LuaVersion, MAX_ACTIVE_LOCALS};

const PUC_VERSIONS: [LuaVersion; 5] =
    [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55];

fn check(source: &str, version: LuaVersion) -> Vec<Diagnostic> {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, source.as_bytes(), version);
    assert!(parsed.errors.is_empty(), "{version:?}: {:?}", parsed.errors);
    let chunk = allocator.alloc(parsed.chunk);
    SemanticBuilder::new().with_counts(parsed.counts).with_check(true).build(chunk).errors
}

/// `local {prefix}1, ..., {prefix}{count}`, each name on its own statement
/// so none waits on another's value.
fn locals(prefix: &str, count: usize) -> String {
    let mut source = String::new();
    for i in 1..=count {
        writeln!(source, "local {prefix}{i}").unwrap();
    }
    source
}

/// What each error says and the text it points at.
fn found<'source>(errors: &[Diagnostic], source: &'source str) -> Vec<(String, &'source str)> {
    errors
        .iter()
        .map(|error| {
            let span = error.span.unwrap();
            (error.message.to_string(), str::from_utf8(span.text(source.as_bytes())).unwrap())
        })
        .collect()
}

#[test]
fn flags_the_local_that_takes_a_function_past_200() {
    let fits = locals("a", MAX_ACTIVE_LOCALS);
    let over = format!("{fits}local last");
    for version in PUC_VERSIONS {
        assert!(check(&fits, version).is_empty(), "{version:?}");
        assert_eq!(
            found(&check(&over, version), &over),
            [("too many local variables (limit is 200) in main function".to_owned(), "last")],
            "{version:?}"
        );
    }
}

#[test]
fn flags_a_function_s_locals_with_the_function() {
    let source = format!("local function f(p)\n{}end", locals("a", MAX_ACTIVE_LOCALS));
    let errors = check(&source, LuaVersion::Lua54);
    let [error] = &*errors else { panic!("{errors:?}") };
    assert_eq!(error.message, "too many local variables (limit is 200) in function");
    assert_eq!(str::from_utf8(error.span.unwrap().text(source.as_bytes())).unwrap(), "a200");
    let labels: Vec<&[u8]> =
        error.labels.iter().map(|label| label.span.text(source.as_bytes())).collect();
    assert_eq!(labels, [b"local function f"], "it points at the function");
}

#[test]
fn counts_each_function_s_locals_apart_and_frees_a_block_s_at_its_end() {
    let over_half = locals("a", 101);
    let source = format!("do\n{over_half}end\n{over_half}local function f()\n{over_half}end");
    for version in PUC_VERSIONS {
        assert!(check(&source, version).is_empty(), "{version:?}");
    }
}

#[test]
fn counts_the_hidden_locals_of_a_for_loop_as_each_version_declares_them() {
    let numeric = format!("{}for i = 1, 2 do end", locals("a", 197));
    let generic = format!("{}for k in next, {{}} do end", locals("a", 196));
    let failing = |source: &str| {
        PUC_VERSIONS.into_iter().filter(|&version| !check(source, version).is_empty()).collect()
    };
    let numeric_failing: Vec<LuaVersion> = failing(&numeric);
    assert_eq!(
        numeric_failing,
        [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54]
    );
    let generic_failing: Vec<LuaVersion> = failing(&generic);
    assert_eq!(generic_failing, [LuaVersion::Lua54]);
}

#[test]
fn counts_a_methods_self_and_lua51_s_arg() {
    let body = locals("a", MAX_ACTIVE_LOCALS - 1);
    let method = format!("local t = {{}}\nfunction t:m()\n{body}end");
    let vararg = format!("local function f(...)\n{body}end");
    for version in PUC_VERSIONS {
        assert_eq!(check(&method, version).len(), 0, "{version:?}: `self` and 199 fit");
    }
    let with_last = |source: &str| source.replace("end", "local last\nend");
    for version in PUC_VERSIONS {
        assert_eq!(check(&with_last(&method), version).len(), 1, "{version:?}");
    }
    let vararg_failing: Vec<LuaVersion> = PUC_VERSIONS
        .into_iter()
        .filter(|&version| !check(&with_last(&vararg), version).is_empty())
        .collect();
    assert_eq!(
        vararg_failing,
        [LuaVersion::Lua51, LuaVersion::Lua55],
        "5.1's `arg` and 5.5's vararg table are locals"
    );
}

#[test]
fn lua55_counts_only_the_locals_in_registers() {
    let source = format!("{}local c <const> = 1\nlocal t <const> = {{}}", locals("a", 199));
    assert!(
        check(&source, LuaVersion::Lua55).is_empty(),
        "a compile-time constant takes no register"
    );
    let leading = source.replace("local c <const>", "local <const> c");
    assert!(check(&leading, LuaVersion::Lua55).is_empty(), "as one a leading `<const>` makes");
    let errors = check(&source, LuaVersion::Lua54);
    assert_eq!(
        found(&errors, &source).first().map(|(_, text)| *text),
        Some("t"),
        "5.4 counts both"
    );
    let global = format!("{}global g", locals("a", MAX_ACTIVE_LOCALS));
    assert!(check(&global, LuaVersion::Lua55).is_empty(), "a global is no local");
}

/// The official suites' test: `fooC` reads 256 variables of the two
/// functions around it.
fn three_functions(outer: usize, middle: usize, reads_global: bool) -> String {
    let mut source = format!("local function fooA()\n{}", locals("a", outer));
    write!(source, "local function fooB()\n{}", locals("b", middle)).unwrap();
    source.push_str("local function fooC()\nreturn 0");
    for i in 1..=outer {
        write!(source, " + a{i}").unwrap();
    }
    for i in 1..=middle {
        write!(source, " + b{i}").unwrap();
    }
    if reads_global {
        source.push_str(" + g");
    }
    source.push_str("\nend end end");
    source
}

#[test]
fn flags_the_use_that_takes_a_function_past_its_upvalues() {
    let fits = three_functions(128, 127, false);
    let over = three_functions(128, 128, false);
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert!(check(&fits, version).is_empty(), "{version:?}");
        assert_eq!(
            found(&check(&over, version), &over),
            [("too many upvalues (limit is 255) in function".to_owned(), "b128")],
            "{version:?}"
        );
    }
    let lua51 = three_functions(30, 31, false);
    assert_eq!(check(&lua51, LuaVersion::Lua51).len(), 1, "5.1 allows 60");
    assert!(check(&three_functions(30, 30, false), LuaVersion::Lua51).is_empty(), "60 fits");
}

#[test]
fn counts_env_as_an_upvalue_from_lua52() {
    let source = three_functions(128, 127, true);
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            found(&check(&source, version), &source).first().map(|(_, text)| *text),
            Some("g"),
            "{version:?}"
        );
    }
    assert!(
        check(&three_functions(30, 30, true), LuaVersion::Lua51).is_empty(),
        "5.1 reads globals by name"
    );
}

#[test]
fn counts_a_capture_in_each_function_between() {
    // `fooB` captures what `fooC` reads of `fooA`, so it passes 60 first.
    let mut source = format!("local function fooA()\n{}", locals("a", 61));
    source.push_str("local function fooB()\nlocal function fooC()\nreturn 0");
    for i in 1..=61 {
        write!(source, " + a{i}").unwrap();
    }
    source.push_str("\nend end end");
    let errors = check(&source, LuaVersion::Lua51);
    let [error] = &*errors else { panic!("{errors:?}") };
    let labels: Vec<&[u8]> =
        error.labels.iter().map(|label| label.span.text(source.as_bytes())).collect();
    assert_eq!(labels, [b"local function fooB"]);
}

#[test]
fn lua54_reads_a_compile_time_constant_as_its_value() {
    let mut source = String::from("local k <const> = 2 ^ 8 // 3\n");
    source.push_str(&three_functions(128, 127, false).replacen("return 0", "return k", 1));
    for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert!(check(&source, version).is_empty(), "{version:?}");
    }
    assert_eq!(check(&source.replace("<const>", ""), LuaVersion::Lua54).len(), 1);
}

#[test]
fn luau_has_no_limits_checked() {
    let over = format!("{}local last", locals("a", MAX_ACTIVE_LOCALS));
    assert!(check(&over, LuaVersion::Luau).is_empty(), "Luau counts locals after it optimizes");
}
