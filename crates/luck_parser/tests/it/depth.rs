use core::iter::repeat_n;

use luck_parser::parse;
use luck_token::LuaVersion;

#[test]
fn documented_resource_boundary_is_dialect_independent() {
    for version in LuaVersion::ALL {
        for depth in [255, 256] {
            let source = format!("return {}1{}", "(".repeat(depth), ")".repeat(depth));
            let result = parse(&source, version);
            if depth == 255 {
                assert!(result.errors.is_empty(), "{version:?}: {:?}", result.errors);
            } else {
                assert!(
                    result
                        .errors
                        .iter()
                        .any(|error| error.message.contains("luck parser resource limit")),
                    "{version:?}: {:?}",
                    result.errors
                );
            }
        }
    }
}

#[test]
fn deeply_nested_parens_no_crash() {
    let open = "(".repeat(1000);
    let close = ")".repeat(1000);
    let source = format!("{open}1{close}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn deeply_nested_binary_ops_no_crash() {
    let additions = repeat_n("1", 3000).collect::<Vec<_>>().join("+");
    let source = format!("local x = {additions}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
}

#[test]
fn nested_blocks_no_crash() {
    let source = format!("{}local x = 1{}", "do ".repeat(500), " end".repeat(500));
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn nested_tables_no_crash() {
    let open = "{".repeat(500);
    let close = "}".repeat(500);
    let source = format!("local x = {open}1{close}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn nested_functions_no_crash() {
    let prefix = "function() return ".repeat(500);
    let suffix = " end".repeat(500);
    let source = format!("local x = {prefix}1{suffix}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn deep_concat_chain_no_crash() {
    // `..` is right-associative, so each operator recurses. This must hit the
    // depth cap, not overflow the stack.
    let concats = repeat_n("\"a\"", 50_000).collect::<Vec<_>>().join("..");
    let source = format!("local x = {concats}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn deep_power_chain_no_crash() {
    let powers = repeat_n("2", 50_000).collect::<Vec<_>>().join("^");
    let source = format!("local x = {powers}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn moderate_concat_chain_parses() {
    let concats = repeat_n("\"a\"", 100).collect::<Vec<_>>().join("..");
    let source = format!("local x = {concats}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
}
