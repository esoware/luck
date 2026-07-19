use luck_token::LuaVersion;

use luck_parser::ParseResult;

fn parse(source: &str, version: LuaVersion) -> ParseResult {
    luck_parser::parse(source, version)
}

#[test]
fn deeply_nested_parens_no_crash() {
    let open = "(".repeat(1000);
    let close = ")".repeat(1000);
    let source = format!("{open}1{close}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(!result.errors.is_empty());
}

#[test]
fn deeply_nested_binary_ops_no_crash() {
    let additions = (0..3000).map(|_| "1").collect::<Vec<_>>().join("+");
    let source = format!("local x = {additions}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
}

#[test]
fn nested_at_exact_limit() {
    let depth = 200;
    let open = "(".repeat(depth);
    let close = ")".repeat(depth);
    let source = format!("local x = {open}1{close}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(
        result.errors.is_empty(),
        "Should succeed at moderate depth: {:?}",
        result.errors
    );
}

#[test]
fn nested_blocks_no_crash() {
    let source = "do ".repeat(500) + "local x = 1" + &" end".repeat(500);
    let result = parse(&source, LuaVersion::Lua54);
    assert!(!result.errors.is_empty());
}

#[test]
fn nested_tables_no_crash() {
    let open = "{".repeat(500);
    let close = "}".repeat(500);
    let source = format!("local x = {open}1{close}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(!result.errors.is_empty());
}

#[test]
fn nested_functions_no_crash() {
    let prefix = "function() return ".repeat(500);
    let suffix = " end".repeat(500);
    let source = format!("local x = {prefix}1{suffix}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(!result.errors.is_empty());
}

#[test]
fn deep_concat_chain_no_crash() {
    // `..` is right-associative: each operator recurses. Must error at the
    // depth cap instead of overflowing the stack.
    let concats = (0..50_000).map(|_| "\"a\"").collect::<Vec<_>>().join("..");
    let source = format!("local x = {concats}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(!result.errors.is_empty());
}

#[test]
fn deep_power_chain_no_crash() {
    let powers = (0..50_000).map(|_| "2").collect::<Vec<_>>().join("^");
    let source = format!("local x = {powers}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(!result.errors.is_empty());
}

#[test]
fn moderate_concat_chain_parses() {
    let concats = (0..100).map(|_| "\"a\"").collect::<Vec<_>>().join("..");
    let source = format!("local x = {concats}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
}
