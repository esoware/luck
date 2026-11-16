use core::ops::Range;

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

/// Stock Lua loops over left-associative operators and suffixes, so a run of
/// them is valid there however long it gets.
///
/// Here each run is one level of nesting, and so is a right-associative one.
#[test]
fn a_run_of_any_length_is_one_level() {
    for (version, source) in [
        (LuaVersion::Lua54, format!("return 1{}", "+1".repeat(200_000))),
        (LuaVersion::Lua54, format!("return a{}", " or a".repeat(200_000))),
        (LuaVersion::Lua54, format!("return a{}", " < a == a".repeat(100_000))),
        (LuaVersion::Lua54, format!("return \"a\"{}", "..\"a\"".repeat(200_000))),
        (LuaVersion::Lua54, format!("return 2{}", "^2".repeat(200_000))),
        (LuaVersion::Lua54, format!("return a{}", ".b".repeat(200_000))),
        (LuaVersion::Lua54, format!("return f{}", "()".repeat(200_000))),
        (LuaVersion::Lua54, format!("return a{}", ":m()".repeat(200_000))),
        (LuaVersion::Lua54, format!("return t{}", "[1]".repeat(200_000))),
        (LuaVersion::Lua54, format!("return f{}", "\"s\"".repeat(200_000))),
        (LuaVersion::Lua54, format!("return a{}", ".b[1]:m()()\"s\"".repeat(50_000))),
        (LuaVersion::Lua54, format!("a{} = 1", ".b".repeat(200_000))),
        (LuaVersion::Lua54, format!("f{}", "()".repeat(200_000))),
        (LuaVersion::Luau, format!("return f{}", "<<T>>".repeat(200_000))),
        (LuaVersion::Luau, format!("a{} += 1", "[1]".repeat(200_000))),
    ] {
        let result = parse(&source, version);
        assert!(result.errors.is_empty(), "{:?}: {:?}", source.get(..20), result.errors.first());
        assert_eq!(block_nesting_depth(&result.block), 2, "{:?}", source.get(..20));
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
        lua("indexes", 1, 1, |n| format!("return {}x{}", "a[".repeat(n), "]".repeat(n))),
        lua("unary operators", 1, 1, |n| format!("return {}x", "not ".repeat(n))),
        lua("runs in parentheses", 1, 2, |n| {
            format!("return {}x{}", "(".repeat(n), "+x)".repeat(n))
        }),
        lua("chains in parentheses", 1, 2, |n| {
            format!("return {}x{}", "(".repeat(n), ").b".repeat(n))
        }),
        lua("functions", 1, 2, |n| {
            format!("return {}x{}", "function() return ".repeat(n), " end".repeat(n))
        }),
        lua("do blocks", 1, 1, |n| format!("{}{}", "do ".repeat(n), "end ".repeat(n))),
        lua("if blocks", 1, 1, |n| format!("{}{}", "if a then ".repeat(n), "end ".repeat(n))),
        lua("else blocks", 1, 1, |n| {
            format!("{}{}", "if a then else ".repeat(n), "end ".repeat(n))
        }),
        lua("loops", 1, 1, |n| format!("{}{}", "while a do ".repeat(n), "end ".repeat(n))),
        lua("operators over parentheses", 2, 1, |n| {
            format!("return {}x{}{}", "(".repeat(n), ")".repeat(n), "+x".repeat(300))
        }),
        lua("fields over parentheses", 2, 1, |n| {
            format!("return {}x{}{}", "(".repeat(n), ")".repeat(n), ".b".repeat(300))
        }),
        lua("fields over a call's arguments", 2, 1, |n| {
            format!("return f({}x{}){}", "(".repeat(n), ")".repeat(n), ".b".repeat(300))
        }),
        lua("operators over a later operand", 2, 1, |n| {
            format!("return x+{}x{}{}", "(".repeat(n), ")".repeat(n), "+x".repeat(300))
        }),
        lua("assignment target over parentheses", 2, 1, |n| {
            format!("{}x{}{} = 1", "(".repeat(n), ")".repeat(n), ".b".repeat(300))
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
        luau("instantiations over parentheses", 2, 1, |n| {
            format!("return {}f{}{}", "(".repeat(n), ")".repeat(n), "<<T>>".repeat(300))
        }),
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
        luau("optionals", 1, 1, |n| format!("type X = T{}", "?".repeat(n))),
        luau("optional over table types", 2, 1, |n| {
            format!("type X = {}T{}?", "{".repeat(n), "}".repeat(n))
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
                assert_eq!(result.nesting_depth, depth, "{}", shape.name);
            } else {
                let [error] = result.errors.as_slice() else {
                    panic!("{} at depth {depth}: {:?}", shape.name, result.errors);
                };
                assert!(error.message.contains("luck parser resource limit"), "{}", shape.name);
                assert!(block_nesting_depth(&result.block) <= MAX_NESTING_DEPTH, "{}", shape.name);
            }
        }
    }
}

/// A run wraps its first operand, so it counts once from that operand's
/// deepest level. Its other operands sit one level in, however many there
/// are.
#[test]
fn a_run_counts_once_from_its_first_operand() {
    for (label, source) in [
        (
            "operators over parentheses",
            format!("return {}x{}{}", "(".repeat(255), ")".repeat(255), "+1".repeat(255)),
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
    let optionals = format!("type X = {}T{}{}", "{".repeat(255), "}".repeat(255), "?".repeat(255));
    assert!(has_depth_limit_error(&parse(optionals, LuaVersion::Luau)));

    let later_operand =
        format!("return 1+{}x{}{}", "(".repeat(254), ")".repeat(254), "+1".repeat(255));
    let result = parse(later_operand, LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "{:?}", result.errors.first());
    assert_eq!(block_nesting_depth(&result.block), MAX_NESTING_DEPTH);
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
fn a_tree_with_errors_stays_within_the_cap() {
    let lua = [
        format!("return {}", "(".repeat(5000)),
        format!("return {}", "{".repeat(5000)),
        format!("return {}", "f(".repeat(5000)),
        format!("return x{}", "+(x".repeat(5000)),
        format!("return x{}", ".b[(x".repeat(5000)),
        format!("return {}x", "- ".repeat(5000)),
        "do ".repeat(5000),
        "function f() ".repeat(5000),
        format!("return {}", "function() return ".repeat(5000)),
    ];
    let luau = [
        format!("return {}x", "if a then b else ".repeat(5000)),
        format!("return {}", "`{".repeat(5000)),
        format!("return {}x{}", "(".repeat(5000), " :: T)".repeat(5000)),
        format!("type X = {}", "{".repeat(5000)),
        format!("type X = {}T", "() -> ".repeat(5000)),
        format!("type X = {}T", "A<".repeat(5000)),
        format!("type X = {}T{}{}", "{".repeat(255), "}".repeat(255), "?".repeat(5000)),
    ];
    let sources = lua.into_iter().map(|source| (LuaVersion::Lua54, source));
    for (version, source) in
        sources.chain(luau.into_iter().map(|source| (LuaVersion::Luau, source)))
    {
        let result = parse(source.as_str(), version);
        assert!(has_depth_limit_error(&result), "{:?}", result.errors.first());
        let mut levels = Levels { depth: 0, deepest: 0 };
        levels.visit_block(&result.block);
        assert!(levels.deepest <= MAX_NESTING_DEPTH, "{}: {:?}", levels.deepest, source.get(..20));
    }
}

/// A misplaced last statement stands as the block around it while that block
/// fits, and is an error node once it wouldn't.
#[test]
fn a_misplaced_last_statement_is_wrapped_only_within_the_cap() {
    let in_blocks = |blocks: usize, statement: &str| {
        format!("{}{statement} x(){}", "while a do ".repeat(blocks), " end".repeat(blocks))
    };
    let returning = |parentheses: usize| {
        format!("return {}a{}", "(".repeat(parentheses), ")".repeat(parentheses))
    };
    for (version, source, wrapped_depth) in [
        (LuaVersion::Lua54, in_blocks(250, &returning(4)), Some(MAX_NESTING_DEPTH)),
        (LuaVersion::Lua54, in_blocks(250, &returning(5)), None),
        (LuaVersion::Lua51, in_blocks(254, "break"), Some(MAX_NESTING_DEPTH)),
        (LuaVersion::Lua51, in_blocks(255, "break"), None),
    ] {
        let result = parse(source.as_str(), version);
        let [error] = result.errors.as_slice() else {
            panic!("{:?}", result.errors);
        };
        assert!(error.message.contains("must be the last statement"), "{error:?}");
        let depth = block_nesting_depth(&result.block);
        assert!(depth <= MAX_NESTING_DEPTH, "{depth}");
        if let Some(expected) = wrapped_depth {
            assert_eq!(depth, expected);
        }
    }
}

/// `construct` as an expression nested at the cap, so anything but a name or
/// a literal there is too deep.
fn at_the_cap_in_an_expression(construct: &str) -> String {
    let depth = MAX_NESTING_DEPTH - 1;
    format!("local v = {}{construct}{}", "(".repeat(depth), ")".repeat(depth))
}

/// The cap ends the parse, so what follows reports nothing more, however
/// broken.
#[test]
fn nesting_past_the_cap_ends_the_parse() {
    let source = format!(
        "{}
local after = 1
x = = 1
",
        at_the_cap_in_an_expression("(x)")
    );
    let result = parse(source.as_str(), LuaVersion::Lua54);
    let [error] = result.errors.as_slice() else {
        panic!("{:?}", result.errors);
    };
    assert!(error.message.contains("luck parser resource limit"), "{error:?}");
    assert_eq!(result.block.stmts.len(), 1, "{:?}", result.block.stmts);
}

/// However many constructs wrap the one the cap stops, it's one error.
#[test]
fn a_construct_past_the_cap_is_one_error_however_many_wrap_it() {
    for (version, construct) in [
        (LuaVersion::Lua54, "(a).b(c) + d .. e"),
        (LuaVersion::Lua54, "-a + b"),
        (LuaVersion::Lua54, "{} == c"),
        (LuaVersion::Lua54, "function() if a then end end"),
        (LuaVersion::Luau, "(a)::T + 1"),
        (LuaVersion::Luau, "if a then b else c :: T"),
        (LuaVersion::Luau, "`{a}b{c}`"),
    ] {
        let source = at_the_cap_in_an_expression(construct);
        let result = parse(source.as_str(), version);
        assert_eq!(result.errors.len(), 1, "{construct}: {:?}", result.errors);
    }
    let source = format!("type X = {}{{T}}? & U | V{}", "{".repeat(254), "}".repeat(254));
    assert_eq!(parse(source.as_str(), LuaVersion::Luau).errors.len(), 1);
}

#[test]
fn an_operand_cut_off_at_the_cap_is_one_error_where_it_was_cut() {
    let deep = format!("{}x{}", "(".repeat(MAX_NESTING_DEPTH), ")".repeat(MAX_NESTING_DEPTH));
    let source = format!("local v = {deep}.b + 1 :: T .. 2");
    let result = parse(source.as_str(), LuaVersion::Luau);
    let [error] = result.errors.as_slice() else {
        panic!("{:?}", result.errors);
    };
    let innermost = "local v = ".len() + MAX_NESTING_DEPTH - 1;
    assert_eq!(Range::<usize>::from(error.span).start, innermost);
}
