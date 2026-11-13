//! Input nested to the parser's cap, and runs far longer than it, have to run
//! through every stage on the smallest stack luck gets embedded on, in an
//! unoptimized build too.

use std::thread;

use luck_formatter::{FormatOptions, format, format_and_verify};
use luck_minifier::TransformConfig;
use luck_token::LuaVersion;

/// What a Windows main thread and a wasm module get by default, half of a
/// spawned Rust thread's.
const STACK_BYTES: usize = 1024 * 1024;

fn wrapped(open: &str, count: usize, innermost: &str, close: &str) -> String {
    format!("{}{innermost}{}", open.repeat(count), close.repeat(count))
}

/// One source per construct, each nested as deep as the parser accepts.
fn sources_at_the_nesting_cap() -> Vec<(LuaVersion, String)> {
    let lua = [
        format!("return {}", wrapped("(", 255, "x", ")")),
        format!("return {}", wrapped("f(", 255, "x", ")")),
        format!("return {}", wrapped("a:m(", 255, "x", ")")),
        format!("return {}", wrapped("a[", 255, "x", "]")),
        format!("return {}", wrapped("{", 255, "1", "}")),
        format!("return {}", wrapped("{a=", 255, "1", "}")),
        format!("return {}", wrapped("{[1]=", 255, "1", "}")),
        format!("return {}", wrapped("f{", 127, "", "}")),
        format!("return {}x", "not ".repeat(255)),
        format!("return {}x{}", "(".repeat(127), "+x)".repeat(127)),
        format!("return {}", wrapped("x..(", 127, "x", ")")),
        format!("return {}", wrapped("x^-", 127, "x", "")),
        format!("return {}a{}", "(".repeat(127), ").b".repeat(127)),
        format!("return {}f{}", "(".repeat(127), ")()".repeat(127)),
        format!("return {}", wrapped("function() return ", 127, "1", " end")),
        wrapped("x = function() ", 127, "x = 1", " end"),
        wrapped("local function f() ", 127, "local x = 1", " end"),
        wrapped("do ", 255, "x = 1", " end"),
        wrapped("if a then ", 255, "x = 1", " end"),
        wrapped("if a then else ", 255, "x = 1", " end"),
        wrapped("if a then elseif b then ", 255, "x = 1", " end"),
        wrapped("while a do ", 255, "x = 1", " end"),
        wrapped("for i = 1, 2 do ", 255, "x = 1", " end"),
        wrapped("for k, v in p do ", 255, "x = 1", " end"),
        wrapped("repeat ", 255, "x = 1", " until a"),
    ];
    let luau = [
        format!("return {}x", "if a then b else ".repeat(255)),
        format!("return {}", wrapped("`{", 255, "x", "}`")),
        format!("return {}", wrapped("(", 127, "x", " :: T)")),
        format!("type X = {}", wrapped("{", 255, "T", "}")),
        format!("type X = {}", wrapped("(", 255, "T", ")")),
        format!("type X = {}", wrapped("A<", 255, "T", " >")),
        format!("type X = {}T", "() -> ".repeat(255)),
        format!("type X = {}", wrapped("(A | ", 127, "T", ")")),
        format!("type X = {}", wrapped("typeof(x :: ", 127, "x", ")")),
        format!("local x: {} = 1", wrapped("{", 255, "T", "}")),
    ];
    lua.into_iter()
        .map(|source| (LuaVersion::Lua54, source))
        .chain(luau.into_iter().map(|source| (LuaVersion::Luau, source)))
        .collect()
}

/// One source per kind of run, each one level of nesting however long.
fn long_runs() -> Vec<(LuaVersion, String)> {
    const LINKS: usize = 20_000;
    let lua = [
        format!("return x{}", "+x".repeat(LINKS)),
        format!("return 1{}", "+1".repeat(LINKS)),
        format!("return x{}", " < x == x".repeat(LINKS)),
        format!("return x{}", " and x or x".repeat(LINKS)),
        format!("return x{}", "..x".repeat(LINKS)),
        format!("return 2{}", "^2".repeat(LINKS)),
        format!("return a{}", ".b".repeat(LINKS)),
        format!("return f{}", "()".repeat(LINKS)),
        format!("return a{}", ":m()".repeat(LINKS)),
        format!("return a{}", ".b[x]:m(x)\"s\"{}".repeat(LINKS)),
        format!("a{} = 1", "[x]".repeat(LINKS)),
        format!("f{}", "()".repeat(LINKS)),
    ];
    let luau =
        [format!("return f{}", "<<T>>".repeat(LINKS)), format!("a{} += 1", ".b".repeat(LINKS))];
    lua.into_iter()
        .map(|source| (LuaVersion::Lua54, source))
        .chain(luau.into_iter().map(|source| (LuaVersion::Luau, source)))
        .collect()
}

/// One source per construct, each nested far past the cap, which the parser
/// skips without recursing.
fn sources_past_the_nesting_cap() -> Vec<(LuaVersion, String)> {
    const LEVELS: usize = 100_000;
    let lua = [
        format!("return {}", wrapped("(", LEVELS, "x", ")")),
        format!("return {}", wrapped("f(", LEVELS, "x", ")")),
        format!("return {}", wrapped("a[", LEVELS, "x", "]")),
        format!("return {}", wrapped("{", LEVELS, "1", "}")),
        format!("return {}", wrapped("f{", LEVELS, "", "}")),
        format!("return {}x", "- ".repeat(LEVELS)),
        format!("return {}", wrapped("x^-", LEVELS, "x", "")),
        format!("return {}", wrapped("function() return ", LEVELS, "1", " end")),
        wrapped("do ", LEVELS, "x = 1", " end"),
        wrapped("if a then ", LEVELS, "x = 1", " end"),
        wrapped("while a do ", LEVELS, "x = 1", " end"),
        wrapped("repeat ", LEVELS, "x = 1", " until a"),
        wrapped("local function f() ", LEVELS, "local x = 1", " end"),
    ];
    let luau = [
        format!("return {}x", "if a then b else ".repeat(LEVELS)),
        format!("return {}a{}", "if ".repeat(LEVELS), " then b else c".repeat(LEVELS)),
        format!("return {}", wrapped("`{", LEVELS, "x", "}`")),
        format!("return {}", wrapped("(", LEVELS, "x", " :: T)")),
        format!("type X = {}", wrapped("{", LEVELS, "T", "}")),
        format!("type X = {}", wrapped("A<", LEVELS, "T", ">")),
        format!("type X = {}T", "() -> ".repeat(LEVELS)),
        format!("type X = {}T", "() -> ...".repeat(LEVELS)),
        format!("type X = T{}", "?".repeat(LEVELS)),
        format!("type X = {}", wrapped("typeof(", LEVELS, "x", ")")),
    ];
    lua.into_iter()
        .map(|source| (LuaVersion::Lua54, source))
        .chain(luau.into_iter().map(|source| (LuaVersion::Luau, source)))
        .collect()
}

/// Runs `stage` over every source, each on a thread with [`STACK_BYTES`] of
/// stack. Overflowing it takes the whole test process down.
fn run_on_the_smallest_stack(stage: fn(&str, LuaVersion)) {
    run_each_on_the_smallest_stack(
        sources_at_the_nesting_cap().into_iter().chain(long_runs()),
        stage,
    );
}

fn run_each_on_the_smallest_stack<Sources>(sources: Sources, stage: fn(&str, LuaVersion))
where
    Sources: IntoIterator<Item = (LuaVersion, String)>,
{
    for (version, source) in sources {
        let worker = thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn(move || stage(&source, version))
            .expect("the thread starts");
        assert!(worker.join().is_ok(), "the stage panicked");
    }
}

#[test]
fn parsing_past_the_cap_fits_the_smallest_stack() {
    run_each_on_the_smallest_stack(sources_past_the_nesting_cap(), |source, version| {
        let parsed = luck_parser::parse(source, version);
        let [error] = parsed.errors.as_slice() else {
            panic!("{:?}: {:?}", source.get(..20), parsed.errors.first());
        };
        assert!(error.message.contains("resource limit"), "{:?}: {error:?}", source.get(..20));
    });
}

#[test]
fn parsing_validating_and_dropping_fit_the_smallest_stack() {
    run_on_the_smallest_stack(|source, version| {
        let parsed = luck_parser::parse(source, version);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors.first());
        let cloned = parsed.block.clone();
        assert!(cloned == parsed.block, "a clone equals its original");
        assert!(luck_parser::validate(&parsed.block, version).is_empty(), "the source is valid");
    });
}

#[test]
fn compact_printing_fits_the_smallest_stack() {
    run_on_the_smallest_stack(|source, version| {
        let parsed = luck_parser::parse(source, version);
        let output = luck_codegen::compact(&parsed.block).expect("a parsed tree prints");
        assert!(luck_parser::parse(output, version).errors.is_empty(), "the output reparses");
    });
}

#[test]
fn formatting_fits_the_smallest_stack() {
    run_on_the_smallest_stack(|source, version| {
        let options = FormatOptions::default();
        let formatted = format(source, version, &options);
        assert!(formatted.errors.is_empty(), "{:?}", formatted.errors.first());
        let again = format(&formatted.output, version, &options);
        assert!(again.output == formatted.output, "formatting is idempotent");
    });
}

#[test]
fn verified_formatting_fits_the_smallest_stack() {
    run_on_the_smallest_stack(|source, version| {
        assert!(format_and_verify(source, version, &FormatOptions::default()).is_ok(), "verified");
    });
}

#[test]
fn minifying_fits_the_smallest_stack() {
    run_on_the_smallest_stack(|source, version| {
        let config = TransformConfig::default();
        let minified = luck_minifier::minify(source, version, &config).expect("the source parses");
        let again = luck_minifier::minify(&minified, version, &config).expect("the output parses");
        assert!(again == minified, "minifying is idempotent");
    });
}
