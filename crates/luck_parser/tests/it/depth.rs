use core::iter::repeat_n;

use luck_parser::parse;
use luck_token::LuaVersion;

#[test]
fn depth_cap_is_the_same_in_every_dialect() {
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
fn parens_past_the_depth_cap_are_an_error() {
    let open = "(".repeat(1000);
    let close = ")".repeat(1000);
    let source = format!("{open}1{close}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn long_left_associative_chain_stays_under_the_depth_cap() {
    let additions = repeat_n("1", 3000).collect::<Vec<_>>().join("+");
    let source = format!("local x = {additions}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
}

#[test]
fn blocks_past_the_depth_cap_are_an_error() {
    let source = format!("{}local x = 1{}", "do ".repeat(500), " end".repeat(500));
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn tables_past_the_depth_cap_are_an_error() {
    let open = "{".repeat(500);
    let close = "}".repeat(500);
    let source = format!("local x = {open}1{close}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn functions_past_the_depth_cap_are_an_error() {
    let prefix = "function() return ".repeat(500);
    let suffix = " end".repeat(500);
    let source = format!("local x = {prefix}1{suffix}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn concat_chain_past_the_depth_cap_is_an_error() {
    // `..` is right-associative, so each operator recurses. This must hit the
    // depth cap, not overflow the stack.
    let concats = repeat_n("\"a\"", 50_000).collect::<Vec<_>>().join("..");
    let source = format!("local x = {concats}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn power_chain_past_the_depth_cap_is_an_error() {
    let powers = repeat_n("2", 50_000).collect::<Vec<_>>().join("^");
    let source = format!("local x = {powers}");
    let result = parse(&source, LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<luck_token::SourceError>::new());
}

#[test]
fn concat_chain_under_the_depth_cap_parses() {
    let concats = repeat_n("\"a\"", 100).collect::<Vec<_>>().join("..");
    let source = format!("local x = {concats}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
}

fn has_depth_limit_error(result: &luck_parser::ParseResult) -> bool {
    result.errors.iter().any(|error| error.message.contains("luck parser resource limit"))
}

#[test]
fn bug_nested_call_arguments_count_toward_the_depth_cap() {
    let source = format!("return {}1{}", "f(".repeat(300), ")".repeat(300));
    let result = parse(&source, LuaVersion::Lua54);
    assert!(has_depth_limit_error(&result), "call nesting past the cap must report the limit");
}

#[test]
fn bug_nested_index_expressions_count_toward_the_depth_cap() {
    let source = format!("return {}1{}", "a[".repeat(300), "]".repeat(300));
    let result = parse(&source, LuaVersion::Lua54);
    assert!(has_depth_limit_error(&result), "index nesting past the cap must report the limit");
}

#[test]
fn bug_nested_interpolated_strings_count_toward_the_depth_cap() {
    let source = format!("return {}1{}", "`{".repeat(300), "}`".repeat(300));
    let result = parse(&source, LuaVersion::Luau);
    assert!(has_depth_limit_error(&result), "string nesting past the cap must report the limit");
}

#[test]
fn bug_nested_attribute_arguments_count_toward_the_depth_cap() {
    let source = format!("return {}1{}", "@[deprecated(".repeat(300), ")]".repeat(300));
    let result = parse(&source, LuaVersion::Luau);
    assert!(has_depth_limit_error(&result), "attribute nesting past the cap must report the limit");
}

/// Fails with a stack overflow in the recursive drop of the tree, which is
/// as deep as the chain is long.
#[test]
fn bug_long_binary_chain_does_not_overflow_the_stack() {
    let source = format!("return 1{}", "+1".repeat(200_000));
    let result = parse(&source, LuaVersion::Lua54);
    assert!(
        result.errors.is_empty() || has_depth_limit_error(&result),
        "a long chain parses or reports the limit: {:?}",
        result.errors.first()
    );
}

/// Fails with a stack overflow in the recursive drop of the tree.
#[test]
fn bug_long_suffix_chain_does_not_overflow_the_stack() {
    let source = format!("return a{}", ".b".repeat(200_000));
    let result = parse(&source, LuaVersion::Lua54);
    assert!(
        result.errors.is_empty() || has_depth_limit_error(&result),
        "a long chain parses or reports the limit: {:?}",
        result.errors.first()
    );
}

/// Fails with a stack overflow in the recursive drop of the tree.
#[test]
fn bug_long_optional_type_chain_does_not_overflow_the_stack() {
    let source = format!("type X = number{}", "?".repeat(200_000));
    let result = parse(&source, LuaVersion::Luau);
    assert!(
        result.errors.is_empty() || has_depth_limit_error(&result),
        "a long chain parses or reports the limit: {:?}",
        result.errors.first()
    );
}
