use core::iter::repeat_n;

use luck_ast::{MAX_NESTING_DEPTH, Visitor, block_nesting_depth};
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
                    result.errors.iter().any(|e| e.message.contains("luck parser resource limit")),
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
fn left_associative_chain_past_the_depth_cap_is_an_error() {
    let additions = repeat_n("1", 3000).collect::<Vec<_>>().join("+");
    let source = format!("local x = {additions}");
    let result = parse(&source, LuaVersion::Lua54);
    assert!(has_depth_limit_error(&result), "{:?}", result.errors.first());
}

#[test]
fn chains_under_the_depth_cap_parse() {
    for (version, source) in [
        (LuaVersion::Lua54, format!("return 1{}", "+1".repeat(200))),
        (LuaVersion::Lua54, format!("return a{}", ".b".repeat(200))),
        (LuaVersion::Lua54, format!("return f{}", "()".repeat(200))),
        (LuaVersion::Lua54, format!("return t{}", "[1]".repeat(200))),
        (LuaVersion::Luau, format!("type X = number{}", "?".repeat(200))),
    ] {
        let result = parse(&source, version);
        assert!(result.errors.is_empty(), "{version:?}: {:?}", result.errors.first());
    }
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
    result.errors.iter().any(|e| e.message.contains("luck parser resource limit"))
}

#[test]
fn nested_call_arguments_count_toward_the_depth_cap() {
    let source = format!("return {}1{}", "f(".repeat(300), ")".repeat(300));
    let result = parse(&source, LuaVersion::Lua54);
    assert!(has_depth_limit_error(&result), "call nesting past the cap must report the limit");
}

#[test]
fn nested_index_expressions_count_toward_the_depth_cap() {
    let source = format!("return {}1{}", "a[".repeat(300), "]".repeat(300));
    let result = parse(&source, LuaVersion::Lua54);
    assert!(has_depth_limit_error(&result), "index nesting past the cap must report the limit");
}

#[test]
fn nested_interpolated_strings_count_toward_the_depth_cap() {
    let source = format!("return {}1{}", "`{".repeat(300), "}`".repeat(300));
    let result = parse(&source, LuaVersion::Luau);
    assert!(has_depth_limit_error(&result), "string nesting past the cap must report the limit");
}

#[test]
fn nested_attribute_arguments_count_toward_the_depth_cap() {
    let source = format!("return {}1{}", "@[deprecated(".repeat(300), ")]".repeat(300));
    let result = parse(&source, LuaVersion::Luau);
    assert!(has_depth_limit_error(&result), "attribute nesting past the cap must report the limit");
}

#[test]
fn long_binary_chain_reports_the_depth_cap() {
    let source = format!("return 1{}", "+1".repeat(200_000));
    let result = parse(&source, LuaVersion::Lua54);
    assert!(has_depth_limit_error(&result), "{:?}", result.errors.first());
}

#[test]
fn long_suffix_chains_report_the_depth_cap() {
    for link in [".b", "()", "[1]", ":m()", "\"s\""] {
        let source = format!("return a{}", link.repeat(200_000));
        let result = parse(&source, LuaVersion::Lua54);
        assert!(has_depth_limit_error(&result), "{link}: {:?}", result.errors.first());
    }
}

#[test]
fn long_optional_type_chain_reports_the_depth_cap() {
    let source = format!("type X = number{}", "?".repeat(200_000));
    let result = parse(&source, LuaVersion::Luau);
    assert!(has_depth_limit_error(&result), "{:?}", result.errors.first());
}

/// A construct nested `wraps` times, and how deep that makes the tree.
struct Shape {
    name: &'static str,
    version: LuaVersion,
    fixed_levels: usize,
    levels_per_wrap: usize,
    build: fn(usize) -> String,
}

const fn lua(
    name: &'static str,
    fixed_levels: usize,
    levels_per_wrap: usize,
    build: fn(usize) -> String,
) -> Shape {
    Shape { name, version: LuaVersion::Lua54, fixed_levels, levels_per_wrap, build }
}

const fn luau(
    name: &'static str,
    fixed_levels: usize,
    levels_per_wrap: usize,
    build: fn(usize) -> String,
) -> Shape {
    Shape { name, version: LuaVersion::Luau, fixed_levels, levels_per_wrap, build }
}

/// The chunk's block is the first level of every shape.
fn shapes() -> Vec<Shape> {
    vec![
        lua("parentheses", 1, 1, |n| format!("return {}x{}", "(".repeat(n), ")".repeat(n))),
        lua("tables", 1, 1, |n| format!("return {}{}", "{".repeat(n), "}".repeat(n))),
        lua("calls", 1, 1, |n| format!("return {}x{}", "f(".repeat(n), ")".repeat(n))),
        lua("bare table calls", 1, 2, |n| format!("return {}{}", "f{".repeat(n), "}".repeat(n))),
        lua("parenthesized table calls", 1, 2, |n| {
            format!("return {}{}", "f({".repeat(n), "})".repeat(n))
        }),
        lua("string calls", 1, 1, |n| format!("return f{}", "\"s\"".repeat(n))),
        lua("indexes", 1, 1, |n| format!("return {}x{}", "a[".repeat(n), "]".repeat(n))),
        lua("unary operators", 1, 1, |n| format!("return {}x", "not ".repeat(n))),
        lua("left-associative operators", 1, 1, |n| format!("return x{}", "+x".repeat(n))),
        lua("right-associative operators", 1, 1, |n| format!("return x{}", "..x".repeat(n))),
        lua("fields", 1, 1, |n| format!("return a{}", ".b".repeat(n))),
        lua("call chain", 1, 1, |n| format!("return f{}", "()".repeat(n))),
        lua("method chain", 1, 1, |n| format!("return a{}", ":m()".repeat(n))),
        lua("index chain", 1, 1, |n| format!("return a{}", "[1]".repeat(n))),
        lua("assignment target", 1, 1, |n| format!("a{} = 1", ".b".repeat(n))),
        lua("call statement", 1, 1, |n| format!("f{}", "()".repeat(n))),
        lua("functions", 1, 2, |n| {
            format!("return {}x{}", "function() return ".repeat(n), " end".repeat(n))
        }),
        lua("do blocks", 1, 1, |n| format!("{}{}", "do ".repeat(n), "end ".repeat(n))),
        lua("if blocks", 1, 1, |n| format!("{}{}", "if a then ".repeat(n), "end ".repeat(n))),
        lua("else blocks", 1, 1, |n| {
            format!("{}{}", "if a then else ".repeat(n), "end ".repeat(n))
        }),
        lua("loops", 1, 1, |n| format!("{}{}", "while a do ".repeat(n), "end ".repeat(n))),
        lua("operators over parentheses", 101, 1, |n| {
            format!("return {}x{}{}", "(".repeat(100), ")".repeat(100), "+x".repeat(n))
        }),
        lua("fields over parentheses", 101, 1, |n| {
            format!("return {}x{}{}", "(".repeat(100), ")".repeat(100), ".b".repeat(n))
        }),
        lua("fields over a call's arguments", 102, 1, |n| {
            format!("return f({}x{}){}", "(".repeat(100), ")".repeat(100), ".b".repeat(n))
        }),
        lua("operators over a right operand", 102, 1, |n| {
            format!("return x+{}x{}{}", "(".repeat(100), ")".repeat(100), "+x".repeat(n))
        }),
        lua("parentheses under a long chain", 2, 1, |n| {
            format!("return x{}+{}x{}", "+x".repeat(50), "(".repeat(n), ")".repeat(n))
        }),
        luau("if expressions", 1, 1, |n| format!("return {}x", "if a then b else ".repeat(n))),
        luau("interpolated strings", 1, 1, |n| {
            format!("return {}x{}", "`{".repeat(n), "}`".repeat(n))
        }),
        luau("plain interpolated string", 2, 1, |n| {
            format!("return {}`s`{}", "(".repeat(n), ")".repeat(n))
        }),
        luau("casts", 1, 2, |n| format!("return {}x{}", "(".repeat(n), " :: T)".repeat(n))),
        luau("cast over parentheses", 2, 1, |n| {
            format!("return {}x{} :: T", "(".repeat(n), ")".repeat(n))
        }),
        luau("instantiations", 1, 1, |n| format!("return f{}", "<<T>>".repeat(n))),
        luau("optional types", 1, 1, |n| format!("type X = T{}", "?".repeat(n))),
        luau("table types", 1, 1, |n| format!("type X = {}T{}", "{".repeat(n), "}".repeat(n))),
        luau("parenthesized types", 1, 1, |n| {
            format!("type X = {}T{}", "(".repeat(n), ")".repeat(n))
        }),
        luau("function types", 1, 1, |n| format!("type X = {}T", "() -> ".repeat(n))),
        luau("generic function types", 1, 1, |n| format!("type X = {}T", "<A>() -> ".repeat(n))),
        luau("generic arguments", 1, 1, |n| {
            format!("type X = {}T{}", "A<".repeat(n), " >".repeat(n))
        }),
        luau("unions in parentheses", 1, 2, |n| {
            format!("type X = {}T{}", "(A | ".repeat(n), ")".repeat(n))
        }),
        luau("leading-pipe unions", 1, 2, |n| {
            format!("type X = {}T{}", "(| A | ".repeat(n), ")".repeat(n))
        }),
        luau("intersections in parentheses", 1, 2, |n| {
            format!("type X = {}T{}", "(A & ".repeat(n), ")".repeat(n))
        }),
        luau("variadic return types", 1, 2, |n| format!("type X = {}T", "() -> ...".repeat(n))),
        luau("optional over a table type", 101, 1, |n| {
            format!("type X = {}T{}{}", "{".repeat(100), "}".repeat(100), "?".repeat(n))
        }),
        luau("table type beside a shallower union member", 2, 1, |n| {
            format!(
                "type X = {}T{} | {}U{}",
                "{".repeat(n),
                "}".repeat(n),
                "(".repeat(9),
                ")".repeat(9)
            )
        }),
        luau("typeof", 1, 2, |n| {
            format!("type X = {}x{}", "typeof(x :: ".repeat(n), ")".repeat(n))
        }),
        luau("annotation", 1, 1, |n| format!("local x: {}T{}", "{".repeat(n), "}".repeat(n))),
        luau("return annotation", 2, 1, |n| {
            format!("local function f(): {}T{} end", "{".repeat(n), "}".repeat(n))
        }),
    ]
}

#[test]
fn the_cap_bounds_the_depth_of_the_tree_exactly() {
    for shape in shapes() {
        let wraps_at_cap =
            (MAX_NESTING_DEPTH - shape.fixed_levels).div_euclid(shape.levels_per_wrap);
        for wraps in wraps_at_cap - 1..=wraps_at_cap + 2 {
            let depth = shape.fixed_levels + shape.levels_per_wrap * wraps;
            let result = parse((shape.build)(wraps), shape.version);
            if depth <= MAX_NESTING_DEPTH {
                assert!(
                    result.errors.is_empty(),
                    "{} at depth {depth}: {:?}",
                    shape.name,
                    result.errors.first()
                );
                assert_eq!(block_nesting_depth(&result.block), depth, "{}", shape.name);
            } else {
                assert!(has_depth_limit_error(&result), "{} at depth {depth}", shape.name);
            }
        }
    }
}

#[test]
fn a_chain_counts_from_its_deepest_operand() {
    for (label, source) in [
        (
            "operators over parentheses",
            format!("return {}x{}{}", "(".repeat(255), ")".repeat(255), "+1".repeat(255)),
        ),
        (
            "operators over a right operand",
            format!("return 1+{}x{}{}", "(".repeat(254), ")".repeat(254), "+1".repeat(255)),
        ),
        (
            "fields over parentheses",
            format!("return {}x{}{}", "(".repeat(255), ")".repeat(255), ".b".repeat(255)),
        ),
        (
            "calls over a table",
            format!("return ({}{}){}", "{".repeat(254), "}".repeat(254), "()".repeat(255)),
        ),
    ] {
        let result = parse(source, LuaVersion::Lua54);
        assert!(has_depth_limit_error(&result), "{label}: {:?}", result.errors.first());
    }
    let optionals = format!("type X = {}T{}{}", "{".repeat(254), "}".repeat(254), "?".repeat(255));
    assert!(has_depth_limit_error(&parse(optionals, LuaVersion::Luau)));
}

#[test]
fn chains_nested_in_parentheses_cannot_compound_past_the_cap() {
    // Level `j` of parentheses is followed by a chain of `255 - j` links.
    let mut source = format!("1{}", "+1".repeat(205));
    for paren_level in (0..50usize).rev() {
        source = format!("({source}){}", "+1".repeat(255 - paren_level));
    }
    let result = parse(format!("return {source}"), LuaVersion::Lua54);
    assert!(has_depth_limit_error(&result), "{:?}", result.errors.first());
}

/// Counts every level, where `block_nesting_depth` stops just past the cap.
struct Levels {
    depth: usize,
    deepest: usize,
}

impl Visitor<'_> for Levels {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        self.deepest = self.deepest.max(self.depth);
        true
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

#[test]
fn a_tree_with_errors_stays_within_reach_of_the_cap() {
    for source in [
        format!("return {}", "(".repeat(5000)),
        format!("return {}", "{".repeat(5000)),
        format!("return {}", "f(".repeat(5000)),
        format!("return x{}", "+(x".repeat(5000)),
        format!("return x{}", "+x".repeat(5000)),
        format!("return x{}", ".b".repeat(5000)),
        format!("return x{}", "+-".repeat(5000)),
        "do ".repeat(5000),
        "function f() ".repeat(5000),
        format!("return {}", "function() return ".repeat(5000)),
    ] {
        let result = parse(source.as_str(), LuaVersion::Lua54);
        assert!(has_depth_limit_error(&result), "{:?}", result.errors.first());
        let mut levels = Levels { depth: 0, deepest: 0 };
        levels.visit_block(&result.block);
        assert!(
            levels.deepest <= MAX_NESTING_DEPTH + 2,
            "{}: {:?}",
            levels.deepest,
            source.get(..20)
        );
    }
}
