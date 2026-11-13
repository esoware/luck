//! Input nested to the parser's cap has to run through every stage on the
//! smallest stack luck gets embedded on, in an unoptimized build too.

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
        format!("return x{}", "+x".repeat(255)),
        format!("return x{}", "..x".repeat(255)),
        format!("return a{}", ".b".repeat(255)),
        format!("return f{}", "()".repeat(255)),
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

/// Runs `stage` over every source, each on a thread with [`STACK_BYTES`] of
/// stack. Overflowing it takes the whole test process down.
fn run_on_the_smallest_stack(stage: fn(&str, LuaVersion)) {
    for (version, source) in sources_at_the_nesting_cap() {
        let worker = thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn(move || stage(&source, version))
            .expect("the thread starts");
        assert!(worker.join().is_ok(), "the stage panicked");
    }
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
