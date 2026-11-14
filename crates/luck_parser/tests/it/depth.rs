use core::ops::Range;

use luck_ast::{MAX_NESTING_DEPTH, Statement, Visitor, block_nesting_depth};
use luck_parser::parse;
use luck_testgen::generate_full_embeddable;
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
/// them is valid there however long it gets. Here each run is one level of
/// nesting, and so is a right-associative one.
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
    let optionals = format!("type X = {}T{}{}", "{".repeat(254), "}".repeat(254), "?".repeat(255));
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
        format!("type X = T{}", "?".repeat(5000)),
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

const AFTER: &str = "\nlocal after = 1";

/// `construct` as an expression nested at the cap, so anything but a name or
/// a literal there is too deep.
fn at_the_cap_in_an_expression(construct: &str) -> (String, String) {
    let prefix = format!("local v = {}", "(".repeat(MAX_NESTING_DEPTH - 1));
    let source = format!("{prefix}{construct}{}{AFTER}", ")".repeat(MAX_NESTING_DEPTH - 1));
    (prefix, source)
}

/// `construct` as an expression nested one level under the cap, so it fits and
/// anything nesting inside it doesn't.
fn under_the_cap_in_an_expression(construct: &str) -> (String, String) {
    let prefix = format!("local v = {}", "(".repeat(MAX_NESTING_DEPTH - 2));
    let source = format!("{prefix}{construct}{}{AFTER}", ")".repeat(MAX_NESTING_DEPTH - 2));
    (prefix, source)
}

/// `construct` as a statement in a block at the cap, so a statement that
/// holds a block or an expression that nests is too deep.
fn at_the_cap_in_a_block(construct: &str) -> (String, String) {
    let prefix = "do ".repeat(MAX_NESTING_DEPTH - 1);
    let source = format!("{prefix}{construct}{}{AFTER}", " end".repeat(MAX_NESTING_DEPTH - 1));
    (prefix, source)
}

/// `construct` as a statement whose own block is at the cap, so a function's
/// body and block don't both fit.
fn under_the_cap_in_a_block(construct: &str) -> (String, String) {
    let prefix = "do ".repeat(MAX_NESTING_DEPTH - 2);
    let source = format!("{prefix}{construct}{}{AFTER}", " end".repeat(MAX_NESTING_DEPTH - 2));
    (prefix, source)
}

/// `construct` as a type nested at the cap.
fn at_the_cap_in_a_type(construct: &str) -> (String, String) {
    let prefix = format!("type X = {}", "{".repeat(MAX_NESTING_DEPTH - 1));
    let source = format!("{prefix}{construct}{}{AFTER}", "}".repeat(MAX_NESTING_DEPTH - 1));
    (prefix, source)
}

/// `construct` after an operand that already reaches the cap, so a node
/// wrapping that operand is too deep.
fn over_an_operand_at_the_cap(construct: &str) -> (String, String) {
    let deep = MAX_NESTING_DEPTH - 1;
    let prefix = format!("local v = {}x{}", "(".repeat(deep), ")".repeat(deep));
    (prefix.clone(), format!("{prefix}{construct}{AFTER}"))
}

/// The same, as the start of a statement.
fn over_a_statement_at_the_cap(construct: &str) -> (String, String) {
    let deep = MAX_NESTING_DEPTH - 1;
    let prefix = format!("{}x{}", "(".repeat(deep), ")".repeat(deep));
    (prefix.clone(), format!("{prefix}{construct}{AFTER}"))
}

/// The same, over a type.
fn over_a_type_at_the_cap(construct: &str) -> (String, String) {
    let deep = MAX_NESTING_DEPTH - 1;
    let prefix = format!("type X = {}T{}", "{".repeat(deep), "}".repeat(deep));
    (prefix.clone(), format!("{prefix}{construct}{AFTER}"))
}

type Build = fn(&str) -> (String, String);

/// Each construct `build` puts at the cap is one error at its `opener`, and
/// the statement after it parses.
fn assert_one_error_at_the_opener(cases: &[(Build, LuaVersion, &str, &str)]) {
    for &(build, version, construct, opener) in cases {
        let (prefix, source) = build(construct);
        let result = parse(source.as_str(), version);
        let [error] = result.errors.as_slice() else {
            panic!("{construct}: {:?}", result.errors);
        };
        assert!(error.message.contains("luck parser resource limit"), "{construct}: {error:?}");
        let offset = construct.find(opener).expect("the opener is in the construct");
        assert_eq!(error.span.text(&source), opener, "{construct}");
        assert_eq!(
            Range::<usize>::from(error.span).start,
            prefix.len().saturating_add(offset),
            "{construct}"
        );
        let [_, Statement::LocalAssignment(after)] = result.block.stmts.as_slice() else {
            panic!("{construct}: {:?}", result.block.stmts);
        };
        assert_eq!(after.names.first().and_then(|name| name.name.identifier()), Some("after"));
    }
}

/// A refused construct is one error at the token that would open the level
/// past the cap, and is skipped whole, so the statement holding it ends where
/// it would have and the next one parses.
#[test]
fn a_construct_past_the_cap_is_one_error_at_its_opener() {
    let lua = LuaVersion::Lua54;
    let luau = LuaVersion::Luau;
    let cases: &[(Build, LuaVersion, &str, &str)] = &[
        (at_the_cap_in_an_expression, lua, "(x)", "("),
        (at_the_cap_in_an_expression, lua, "{f = function() if a then end end}", "{"),
        (at_the_cap_in_an_expression, lua, "-a ^ -b ^ c", "-"),
        (at_the_cap_in_an_expression, lua, "not a == b", "not"),
        (at_the_cap_in_an_expression, lua, "#{1, 2} .. x", "#"),
        (at_the_cap_in_an_expression, lua, "a + b * c", "+"),
        (at_the_cap_in_an_expression, lua, "a.b[c]:d(e){f}\"g\" or h", "."),
        (at_the_cap_in_an_expression, lua, "function() return (1) end", "function"),
        (at_the_cap_in_an_expression, luau, "if a then b elseif c then d else e .. f", "if"),
        (at_the_cap_in_an_expression, luau, "if if a then b else c then d else e", "if"),
        (at_the_cap_in_an_expression, luau, "`a{b}c{`d{e}`}`", "`a{"),
        (at_the_cap_in_an_expression, luau, "`plain`", "`plain`"),
        (at_the_cap_in_an_expression, luau, "`{f(a}{b[c}`", "`{"),
        (under_the_cap_in_an_expression, luau, "`{(a}`", "("),
        (under_the_cap_in_an_expression, luau, "`{f<<A<B}{c}`", "<"),
        (under_the_cap_in_an_expression, luau, "`{function() return a}{b}`", "function"),
        (at_the_cap_in_an_expression, luau, "a :: (B) -> C | D", "::"),
        (at_the_cap_in_an_expression, luau, "f<<A<B>>>(c)", "<"),
        (at_the_cap_in_an_expression, luau, "@native @[deprecated{use = 'x'}] function() end", "@"),
        (at_the_cap_in_a_block, lua, "do local t = {[1] = function() end} end", "do"),
        (at_the_cap_in_a_block, lua, "while (function() return true end)() do break end", "while"),
        (at_the_cap_in_a_block, lua, "for i = 1, f(2) do end", "for"),
        (at_the_cap_in_a_block, lua, "for _ in function() end do end", "for"),
        (at_the_cap_in_a_block, lua, "if a then elseif b then else end", "if"),
        (at_the_cap_in_a_block, lua, "repeat local y = 1 until f(y) == 2", "repeat"),
        (at_the_cap_in_a_block, lua, "local function f() end", "function"),
        (at_the_cap_in_a_block, lua, "function a.b:c() end", "function"),
        (at_the_cap_in_a_block, lua, "f(1)", "("),
        (at_the_cap_in_a_block, lua, "a.b:c(1)", "."),
        (at_the_cap_in_a_block, lua, "a.b, c = 1, 2", "."),
        (at_the_cap_in_a_block, lua, "x = -1", "-"),
        (at_the_cap_in_a_block, lua, "do goto l ::l:: end", "do"),
        (at_the_cap_in_a_block, LuaVersion::Lua55, "global function g() end", "function"),
        (at_the_cap_in_a_block, luau, "a.b += 1", "."),
        (at_the_cap_in_a_block, luau, "while if a then b else c do end", "while"),
        (at_the_cap_in_a_block, luau, "if if a then b else c then end", "if"),
        (at_the_cap_in_a_block, luau, "repeat until if a then b else c", "repeat"),
        (
            at_the_cap_in_a_block,
            luau,
            "do local y = if a then function() if b then end end else nil; if c then end end",
            "do",
        ),
        (at_the_cap_in_a_block, luau, "do local s = `{if a then \"x\" else \"y\"}` end", "do"),
        (at_the_cap_in_a_block, luau, "do local x: {[string]: (A<B<C>>) -> D?} = nil end", "do"),
        (at_the_cap_in_a_block, luau, "do x = y :: typeof(if a then b else c) end", "do"),
        (
            at_the_cap_in_a_block,
            luau,
            "local function f<T>(x: Array<T>): Array<T> if x then end return x end",
            "function",
        ),
        (at_the_cap_in_a_block, luau, "type function t(x) return x end", "function"),
        (at_the_cap_in_a_block, luau, "@native local function f() end", "@"),
        (under_the_cap_in_a_block, lua, "local f = function() end", "function"),
        (under_the_cap_in_a_block, luau, "const function f() end", "function"),
        (under_the_cap_in_a_block, luau, "local x: A<{{T}}, B> = 1", "{"),
        (under_the_cap_in_a_block, luau, "local x: ({{T}}, B) -> C = nil", "{"),
        (under_the_cap_in_a_block, luau, "type P<T... = ({{T}})> = T", "{"),
        (at_the_cap_in_a_type, luau, "A<B<C>>", "A"),
        (at_the_cap_in_a_type, luau, "(A, ...B) -> ...C", "("),
        (at_the_cap_in_a_type, luau, "<T>(T) -> T | U", "<"),
        (at_the_cap_in_a_type, luau, "typeof(a < b)", "typeof"),
        (at_the_cap_in_a_type, luau, "A? | B & C", "?"),
        (at_the_cap_in_a_type, luau, "| A | B", "|"),
        (at_the_cap_in_a_type, luau, "{read x: number, write y: string}", "{"),
        (over_an_operand_at_the_cap, lua, " + 1 - 2", "+"),
        (over_an_operand_at_the_cap, lua, " * 2 + 3", "*"),
        (over_an_operand_at_the_cap, lua, ".a.b(1):c()", "."),
        (over_an_operand_at_the_cap, luau, " :: T + 1", "::"),
        (over_a_statement_at_the_cap, lua, ".a = 1", "."),
        (over_a_statement_at_the_cap, lua, "(1)", "("),
        (over_a_type_at_the_cap, luau, "? | U", "?"),
        (over_a_type_at_the_cap, luau, " & U | V", "&"),
    ];
    assert_one_error_at_the_opener(cases);
}

/// A skipped construct ends where parsing it would have, broken or not.
#[test]
fn a_construct_past_the_cap_ends_where_parsing_it_would_have() {
    let lua = LuaVersion::Lua54;
    let luau = LuaVersion::Luau;
    let cases: &[(Build, LuaVersion, &str, &str)] = &[
        // A `>` that ends a type ends an operand, and one that compares
        // leaves an operand to come, so only the second makes `if` an
        // expression.
        (at_the_cap_in_a_block, luau, "do local y = a > if b then c else d end", "do"),
        (at_the_cap_in_a_block, luau, "while a > if b then c else d do end", "while"),
        (at_the_cap_in_a_block, luau, "do x = y :: A<B> > if c then d else e end", "do"),
        (at_the_cap_in_a_block, luau, "do local x: Array<T> if c then end end", "do"),
        (at_the_cap_in_a_block, luau, "do local a: A<B>, b: C<D> if c then end end", "do"),
        (at_the_cap_in_a_block, luau, "do for i: A<B> = 1, 2 do if c then end end end", "do"),
        (at_the_cap_in_a_block, luau, "do type X<T> = A<T> if c then end end", "do"),
        (at_the_cap_in_a_block, luau, "do type X = <T>(T) -> A<T> if c then end end", "do"),
        (at_the_cap_in_a_block, luau, "do f<<A<B>>>(c) if d then end end", "do"),
        (
            at_the_cap_in_a_block,
            luau,
            "local function f<T>(x: T): A<T> if x then end return x end",
            "function",
        ),
        (
            at_the_cap_in_a_block,
            luau,
            "do local f = function(): (A<B>) -> C<D> if c then end end end",
            "do",
        ),
        // A closer closes the innermost construct it can, and everything
        // still open inside it, and a stray closing bracket closes the
        // innermost bracket.
        (at_the_cap_in_a_block, lua, "do f(x end", "do"),
        (at_the_cap_in_a_block, lua, "do t[f(x] end", "do"),
        (at_the_cap_in_a_block, lua, "do x = f(function() y ) end", "do"),
        (at_the_cap_in_a_block, lua, "do x = 1 ) y = 2 end", "do"),
        (at_the_cap_in_a_block, lua, "if a then f(x else g() end", "if"),
        (at_the_cap_in_a_block, lua, "repeat f(x until y", "repeat"),
        (at_the_cap_in_a_block, lua, "while f(x do end", "while"),
        (at_the_cap_in_an_expression, lua, "{1, 2", "{"),
        (at_the_cap_in_an_expression, lua, "(x]", "("),
        (at_the_cap_in_an_expression, lua, "{f(x}", "{"),
        (at_the_cap_in_an_expression, luau, "`{f(x}`", "`{"),
    ];
    assert_one_error_at_the_opener(cases);
}

/// A generated program reaches the whole grammar, so skipping one whole
/// inside each kind of refused construct has to end exactly where parsing it
/// would have.
#[test]
fn a_refused_construct_holding_any_program_ends_where_parsing_would_have() {
    for version in LuaVersion::ALL {
        for seed in 0..24 {
            let program = generate_full_embeddable(seed, version, 30);
            let mut wrappers = vec![
                at_the_cap_in_a_block(&format!(
                    "do
{program}
end"
                )),
                at_the_cap_in_a_block(&format!(
                    "repeat
{program}
until x"
                )),
                at_the_cap_in_a_block(&format!(
                    "while x do
{program}
end"
                )),
                at_the_cap_in_a_block(&format!(
                    "if x then
{program}
else
{program}
end"
                )),
                at_the_cap_in_an_expression(&format!(
                    "function(...)
{program}
end"
                )),
                at_the_cap_in_an_expression(&format!(
                    "{{function(...)
{program}
end}}"
                )),
            ];
            if version.has_interpolated_strings() {
                wrappers.push(at_the_cap_in_an_expression(&format!(
                    "`a{{function(...)
{program}
end}}b`"
                )));
            }
            for (_, source) in wrappers {
                let result = parse(source.as_str(), version);
                let [error] = result.errors.as_slice() else {
                    panic!("{version:?} seed {seed}: {:?} in {program:?}", result.errors);
                };
                assert!(error.message.contains("luck parser resource limit"), "{error:?}");
                let Some(Statement::LocalAssignment(after)) = result.block.stmts.last() else {
                    panic!(
                        "{version:?} seed {seed}: {:?} in {program:?}",
                        result.block.stmts.last()
                    );
                };
                assert_eq!(
                    after.names.first().and_then(|name| name.name.identifier()),
                    Some("after"),
                    "{version:?} seed {seed} in {program:?}"
                );
            }
        }
    }
}

/// An unterminated string ends at its line break, so one refused at the cap is
/// skipped through there and the rest of the file parses.
#[test]
fn an_unterminated_string_past_the_cap_ends_at_its_line() {
    let (_, source) = at_the_cap_in_a_block("local s = `{a}b\n");
    let result = parse(source.as_str(), LuaVersion::Luau);
    let errors: Vec<_> =
        result.errors.iter().map(|e| (e.message.as_str(), e.span.text(&source))).collect();
    let depth_limit =
        format!("maximum nesting depth exceeded (luck parser resource limit: {MAX_NESTING_DEPTH})");
    assert_eq!(errors, [(depth_limit.as_str(), "`{"), ("unterminated interpolated string", "}b")]);
    let [_, Statement::LocalAssignment(after)] = result.block.stmts.as_slice() else {
        panic!("{:?}", result.block.stmts);
    };
    assert_eq!(after.names.first().and_then(|name| name.name.identifier()), Some("after"));
}

/// A wrapper over an operand that holds an error adds no error of its own, so
/// a construct cut off at the cap is one error however many wrap it.
#[test]
fn a_wrapper_over_a_refused_operand_adds_no_error() {
    for (version, construct) in [
        (LuaVersion::Lua54, "(a).b(c) + d .. e"),
        (LuaVersion::Lua54, "-a + b"),
        (LuaVersion::Lua54, "{} == c"),
        (LuaVersion::Luau, "(a)::T + 1"),
        (LuaVersion::Luau, "if a then b else c :: T"),
    ] {
        let (_, source) = at_the_cap_in_an_expression(construct);
        let result = parse(source.as_str(), version);
        assert_eq!(result.errors.len(), 1, "{construct}: {:?}", result.errors);
    }
    let (_, source) = at_the_cap_in_a_type("{T}? & U | V");
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

/// Each construct past the cap is its own error, as long as fixing one
/// leaves the other too deep.
#[test]
fn separate_constructs_past_the_cap_are_separate_errors() {
    for construct in ["print(1) print(2)", "local v, w = f(1), g(2)", "a.b, c[1] = 1, 2"] {
        let (_, source) = at_the_cap_in_a_block(construct);
        let result = parse(source.as_str(), LuaVersion::Lua54);
        assert_eq!(result.errors.len(), 2, "{construct}: {:?}", result.errors);
    }
}
