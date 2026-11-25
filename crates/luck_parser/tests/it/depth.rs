use std::ops::Range;

use luck_allocator::Allocator;
use luck_ast::{MAX_NESTING_DEPTH, Statement, Visit, block_nesting_depth};
use luck_parser::{ParseResult, parse};
use luck_testgen::{
    generate_full_grammar, generate_runtime_safe, lua_fixtures, nested, with_the_smallest_stack,
};
use luck_token::LuaVersion;

fn has_depth_limit_error(result: &ParseResult<'_>) -> bool {
    result.errors.iter().any(|e| e.message.contains("luck parser resource limit"))
}

#[test]
fn depth_cap_is_the_same_in_every_dialect() {
    with_the_smallest_stack(|| {
        for version in LuaVersion::ALL {
            // The chunk's block is a level, and each negation another.
            for negations in [MAX_NESTING_DEPTH - 1, MAX_NESTING_DEPTH] {
                let allocator = Allocator::new();
                let source = format!("return {}1", "not ".repeat(negations));
                let result = parse(&allocator, source.as_bytes(), version);
                if negations < MAX_NESTING_DEPTH {
                    assert!(result.errors.is_empty(), "{version:?}: {:?}", result.errors);
                } else {
                    assert!(has_depth_limit_error(&result), "{version:?}: {:?}", result.errors);
                }
            }
        }
    });
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
        let allocator = Allocator::new();
        let result = parse(&allocator, source.as_bytes(), version);
        assert!(result.errors.is_empty(), "{:?}: {:?}", source.get(..20), result.errors.first());
        assert_eq!(block_nesting_depth(&result.chunk.block), 2, "{:?}", source.get(..20));
    }
}

#[test]
fn nested_attribute_arguments_count_toward_the_depth_cap() {
    with_the_smallest_stack(|| {
        let allocator = Allocator::new();
        let levels = MAX_NESTING_DEPTH;
        let source = format!("return {}1{}", "@[deprecated(".repeat(levels), ")]".repeat(levels));
        let result = parse(&allocator, source.as_bytes(), LuaVersion::Luau);
        assert!(
            has_depth_limit_error(&result),
            "attribute nesting past the cap must report the limit"
        );
    });
}

/// A construct nested `wraps` times, and how deep that makes the tree.
struct Shape {
    name: &'static str,
    version: LuaVersion,
    fixed_levels: usize,
    levels_per_wrap: usize,
    /// Whether each wrap opens parentheses the tree leaves out, which open no
    /// level and have a cap of their own.
    is_grouped: bool,
    build: fn(usize) -> String,
}

const fn lua(
    name: &'static str,
    fixed_levels: usize,
    levels_per_wrap: usize,
    build: fn(usize) -> String,
) -> Shape {
    let version = LuaVersion::Lua54;
    Shape { name, version, fixed_levels, levels_per_wrap, is_grouped: false, build }
}

const fn luau(
    name: &'static str,
    fixed_levels: usize,
    levels_per_wrap: usize,
    build: fn(usize) -> String,
) -> Shape {
    let version = LuaVersion::Luau;
    Shape { name, version, fixed_levels, levels_per_wrap, is_grouped: false, build }
}

const fn grouped(
    name: &'static str,
    version: LuaVersion,
    fixed_levels: usize,
    build: fn(usize) -> String,
) -> Shape {
    Shape { name, version, fixed_levels, levels_per_wrap: 0, is_grouped: true, build }
}

/// The chunk's block is the first level of every shape.
fn shapes() -> Vec<Shape> {
    let (lua54, luau_version) = (LuaVersion::Lua54, LuaVersion::Luau);
    vec![
        grouped("parentheses", lua54, 1, |n| format!("return {}x{}", "(".repeat(n), ")".repeat(n))),
        lua("truncated calls", 1, 2, |n| format!("return {}x{}", "(f(".repeat(n), "))".repeat(n))),
        lua("tables", 1, 1, |n| format!("return {}{}", "{".repeat(n), "}".repeat(n))),
        lua("calls", 1, 1, |n| format!("return {}x{}", "f(".repeat(n), ")".repeat(n))),
        lua("bare table calls", 1, 2, |n| format!("return {}{}", "f{".repeat(n), "}".repeat(n))),
        lua("parenthesized table calls", 1, 2, |n| {
            format!("return {}{}", "f({".repeat(n), "})".repeat(n))
        }),
        lua("indexes", 1, 1, |n| format!("return {}x{}", "a[".repeat(n), "]".repeat(n))),
        lua("unary operators", 1, 1, |n| format!("return {}x", "not ".repeat(n))),
        lua("runs in parentheses", 1, 1, |n| {
            format!("return {}x{}", "(".repeat(n), "+x)".repeat(n))
        }),
        grouped("chains in parentheses", lua54, 2, |n| {
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
        grouped("operators over parentheses", lua54, 2, |n| {
            format!("return {}x{}{}", "(".repeat(n), ")".repeat(n), "+x".repeat(300))
        }),
        lua("operators over negations", 2, 1, |n| {
            format!("return {}x{}", "not ".repeat(n), "+x".repeat(300))
        }),
        grouped("fields over parentheses", lua54, 2, |n| {
            format!("return {}x{}{}", "(".repeat(n), ")".repeat(n), ".b".repeat(300))
        }),
        lua("fields over a truncated call", 4, 1, |n| {
            format!("return (f({}x)){}", "not ".repeat(n), ".b".repeat(300))
        }),
        grouped("fields over a call's arguments", lua54, 2, |n| {
            format!("return f({}x{}){}", "(".repeat(n), ")".repeat(n), ".b".repeat(300))
        }),
        grouped("operators over a later operand", lua54, 2, |n| {
            format!("return x+{}x{}{}", "(".repeat(n), ")".repeat(n), "+x".repeat(300))
        }),
        lua("operators over a later negation", 2, 1, |n| {
            format!("return x+{}x{}", "not ".repeat(n), "+x".repeat(300))
        }),
        grouped("assignment target over parentheses", lua54, 2, |n| {
            format!("{}x{}{} = 1", "(".repeat(n), ")".repeat(n), ".b".repeat(300))
        }),
        luau("if expressions", 1, 1, |n| format!("return {}x", "if a then b else ".repeat(n))),
        luau("interpolated strings", 1, 1, |n| {
            format!("return {}x{}", "`{".repeat(n), "}`".repeat(n))
        }),
        grouped("plain interpolated string", luau_version, 2, |n| {
            format!("return {}`s`{}", "(".repeat(n), ")".repeat(n))
        }),
        luau("casts", 1, 1, |n| format!("return {}x{}", "(".repeat(n), " :: T)".repeat(n))),
        grouped("cast over parentheses", luau_version, 2, |n| {
            format!("return {}x{} :: T", "(".repeat(n), ")".repeat(n))
        }),
        grouped("instantiations over parentheses", luau_version, 2, |n| {
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
        luau("optionals in parentheses", 1, 2, |n| {
            format!("type X = {}T{}", "(".repeat(n), "?)".repeat(n))
        }),
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
    with_the_smallest_stack(|| {
        for shape in shapes() {
            let wraps_at_cap = if shape.is_grouped {
                MAX_NESTING_DEPTH
            } else {
                (MAX_NESTING_DEPTH - shape.fixed_levels) / shape.levels_per_wrap
            };
            for wraps in wraps_at_cap - 1..=wraps_at_cap + 2 {
                let depth = shape.fixed_levels + shape.levels_per_wrap * wraps;
                let allocator = Allocator::new();
                let source = (shape.build)(wraps);
                let result = parse(&allocator, source.as_bytes(), shape.version);
                if wraps <= wraps_at_cap {
                    assert!(
                        result.errors.is_empty(),
                        "{} at depth {depth}: {:?}",
                        shape.name,
                        result.errors.first()
                    );
                    assert_eq!(block_nesting_depth(&result.chunk.block), depth, "{}", shape.name);
                    assert_eq!(result.nesting_depth, depth, "{}", shape.name);
                } else {
                    let [error] = result.errors.as_slice() else {
                        panic!("{} at depth {depth}: {:?}", shape.name, result.errors);
                    };
                    assert!(error.message.contains("luck parser resource limit"), "{}", shape.name);
                    assert!(
                        block_nesting_depth(&result.chunk.block) <= MAX_NESTING_DEPTH,
                        "{}",
                        shape.name
                    );
                }
            }
        }
    });
}

/// Each construct nested as deep as Luau's parser reads it.
///
/// Luau counts a level for each statement in a block and each expression,
/// and stops past 1,000. Each of these takes at most two levels here for each
/// of its own, so the deepest, 1,000 function declarations, sets the cap.
/// Lua's stops at 200.
#[test]
fn each_construct_nests_as_deep_as_luau_reads_it() {
    with_the_smallest_stack(|| {
        let mut deepest = 0;
        for source in [
            nested("local function f() ", 1000, "", " end"),
            nested("function f() ", 1000, "", " end"),
            nested("f{function() ", 500, "", " end}"),
            nested("(function() ", 500, "", " end)()"),
            format!("local v = {}", nested("f{", 999, "", "}")),
            format!("local v = {}x{}", "(".repeat(997), " + 1)".repeat(997)),
            format!("local v = {}x{}", "(".repeat(997), " :: T)".repeat(997)),
            format!("local v = {}f{}", "(".repeat(998), ")()".repeat(998)),
        ] {
            let allocator = Allocator::new();
            let result = parse(&allocator, source.as_bytes(), LuaVersion::Luau);
            assert!(
                result.errors.is_empty(),
                "{:?}: {:?}",
                source.get(..24),
                result.errors.first()
            );
            deepest = deepest.max(block_nesting_depth(&result.chunk.block));
        }
        assert_eq!(deepest, MAX_NESTING_DEPTH);
    });
}

/// A run wraps its first operand, so it counts once from that operand's
/// deepest level. Its other operands sit one level in, however many there
/// are.
#[test]
fn a_run_counts_once_from_its_first_operand() {
    with_the_smallest_stack(|| {
        let allocator = Allocator::new();
        // Under the chunk's block, each operand alone sits at the cap.
        let (groups, tables) = (MAX_NESTING_DEPTH - 1, MAX_NESTING_DEPTH - 2);
        let truncated = |suffixes: &str| {
            let call_tables = MAX_NESTING_DEPTH - 3;
            format!("return (f{}{}){suffixes}", "{".repeat(call_tables), "}".repeat(call_tables))
        };
        let source_at_the_cap = truncated("");
        let at_the_cap = parse(&allocator, source_at_the_cap.as_bytes(), LuaVersion::Lua54);
        assert!(at_the_cap.errors.is_empty(), "{:?}", at_the_cap.errors.first());
        assert_eq!(block_nesting_depth(&at_the_cap.chunk.block), MAX_NESTING_DEPTH);
        for (label, source) in [
            (
                "operators over negations",
                format!("return {}x{}", "not ".repeat(groups), "+1".repeat(300)),
            ),
            ("fields over a truncated call", truncated(&".b".repeat(300))),
        ] {
            let result = parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
            assert!(has_depth_limit_error(&result), "{label}: {:?}", result.errors.first());
        }
        let optionals =
            format!("type X = {}T{}{}", "{".repeat(groups), "}".repeat(groups), "?".repeat(300));
        assert!(has_depth_limit_error(&parse(&allocator, optionals.as_bytes(), LuaVersion::Luau)));
        let bare_optionals = format!("type X = T{}", "?".repeat(5000));
        let optional_run = parse(&allocator, bare_optionals.as_bytes(), LuaVersion::Luau);
        assert!(optional_run.errors.is_empty(), "{:?}", optional_run.errors.first());
        assert_eq!(block_nesting_depth(&optional_run.chunk.block), 2);

        let later_operand = format!("return 1+{}x{}", "not ".repeat(tables), "+1".repeat(300));
        let result = parse(&allocator, later_operand.as_bytes(), LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "{:?}", result.errors.first());
        assert_eq!(block_nesting_depth(&result.chunk.block), MAX_NESTING_DEPTH);
    });
}

/// Counts every level, where `block_nesting_depth` stops just past the cap.
struct Levels {
    depth: usize,
    deepest: usize,
}

impl Visit<'_> for Levels {
    fn descend(&mut self) -> bool {
        self.depth += 1;
        self.deepest = self.deepest.max(self.depth);
        true
    }

    fn ascend(&mut self) {
        self.depth -= 1;
    }
}

#[test]
fn a_tree_with_errors_stays_within_the_cap() {
    with_the_smallest_stack(|| {
        let allocator = Allocator::new();
        let groups = MAX_NESTING_DEPTH - 1;
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
            format!("type X = {}T{}{}", "{".repeat(groups), "}".repeat(groups), "?".repeat(5000)),
        ];
        let sources = lua.into_iter().map(|source| (LuaVersion::Lua54, source));
        for (version, source) in
            sources.chain(luau.into_iter().map(|source| (LuaVersion::Luau, source)))
        {
            let result = parse(&allocator, source.as_bytes(), version);
            assert!(has_depth_limit_error(&result), "{:?}", result.errors.first());
            let mut levels = Levels { depth: 0, deepest: 0 };
            levels.visit_block(&result.chunk.block);
            assert!(
                levels.deepest <= MAX_NESTING_DEPTH,
                "{}: {:?}",
                levels.deepest,
                source.get(..20)
            );
        }
    });
}

/// An `if` the cap stops in its second `elseif` takes its first clause with
/// it, rather than leaving it to the `if` around it.
#[test]
fn an_if_cut_off_at_the_cap_keeps_its_clauses_to_itself() {
    with_the_smallest_stack(|| {
        let allocator = Allocator::new();
        let source = format!(
            "if a then elseif b then {}if c then elseif d then x() elseif e then {}x() end",
            "do ".repeat(MAX_NESTING_DEPTH - 6),
            "do ".repeat(20),
        );
        let result = parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        assert!(has_depth_limit_error(&result), "{:?}", result.errors);
        let Some(Statement::IfStatement(outer)) = result.chunk.block.stmts.first() else {
            panic!("{:?}", result.chunk.block.stmts.first());
        };
        assert_eq!(outer.elseif_clauses.len(), 1, "{:?}", outer.elseif_clauses);
    });
}

/// A misplaced last statement stays in its block, where it was written, so
/// it nests no deeper than it reads.
#[test]
fn a_misplaced_last_statement_nests_where_it_was_written() {
    with_the_smallest_stack(|| {
        let allocator = Allocator::new();
        let in_blocks = |blocks: usize, statement: &str| {
            format!("{}{statement} x(){}", "while a do ".repeat(blocks), " end".repeat(blocks))
        };
        let returning = |negations: usize| format!("return {}a", "not ".repeat(negations));
        for (version, source) in [
            (LuaVersion::Lua54, in_blocks(MAX_NESTING_DEPTH - 6, &returning(5))),
            (LuaVersion::Lua51, in_blocks(MAX_NESTING_DEPTH - 1, "break")),
        ] {
            let result = parse(&allocator, source.as_bytes(), version);
            let [error] = result.errors.as_slice() else {
                panic!("{:?}", result.errors);
            };
            assert!(error.message.contains("must be the last statement"), "{error:?}");
            assert_eq!(result.nesting_depth, MAX_NESTING_DEPTH);
            assert_eq!(block_nesting_depth(&result.chunk.block), MAX_NESTING_DEPTH);
        }
    });
}

/// `construct` as an expression nested at the cap, so anything but a name or
/// a literal there, in parentheses or not, is too deep.
fn at_the_cap_in_an_expression(construct: &str) -> String {
    let depth = MAX_NESTING_DEPTH - 1;
    format!("local v = {}{construct}{}", "{".repeat(depth), "}".repeat(depth))
}

/// The cap ends the parse, so what follows reports nothing more, however
/// broken.
#[test]
fn nesting_past_the_cap_ends_the_parse() {
    with_the_smallest_stack(|| {
        let allocator = Allocator::new();
        let source = format!(
            "{}
    local after = 1
    x = = 1
    ",
            at_the_cap_in_an_expression("{x}")
        );
        let result = parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        let [error] = result.errors.as_slice() else {
            panic!("{:?}", result.errors);
        };
        assert!(error.message.contains("luck parser resource limit"), "{error:?}");
        assert_eq!(result.chunk.block.stmts.len(), 1, "{:?}", result.chunk.block.stmts);
    });
}

/// However many constructs wrap the one the cap stops, it's one error.
#[test]
fn a_construct_past_the_cap_is_one_error_however_many_wrap_it() {
    with_the_smallest_stack(|| {
        let allocator = Allocator::new();
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
            let result = parse(&allocator, source.as_bytes(), version);
            assert_eq!(result.errors.len(), 1, "{construct}: {:?}", result.errors);
        }
        let tables = MAX_NESTING_DEPTH - 2;
        let source = format!("type X = {}{{T}}? & U | V{}", "{".repeat(tables), "}".repeat(tables));
        assert_eq!(parse(&allocator, source.as_bytes(), LuaVersion::Luau).errors.len(), 1);
    });
}

#[test]
fn an_operand_cut_off_at_the_cap_is_one_error_where_it_was_cut() {
    with_the_smallest_stack(|| {
        let allocator = Allocator::new();
        let groups = MAX_NESTING_DEPTH + 1;
        let deep = format!("{}x{}", "(".repeat(groups), ")".repeat(groups));
        let source = format!("local v = {deep}.b + 1 :: T .. 2");
        let result = parse(&allocator, source.as_bytes(), LuaVersion::Luau);
        let [error] = result.errors.as_slice() else {
            panic!("{:?}", result.errors);
        };
        let innermost = "local v = ".len() + MAX_NESTING_DEPTH;
        assert_eq!(Range::<usize>::from(error.span.unwrap()).start, innermost);
    });
}

/// The depth the parser counted and the depth of the tree it built.
fn counted_and_built(source: &str, version: LuaVersion) -> (usize, usize) {
    let allocator = Allocator::new();
    let result = parse(&allocator, source.as_bytes(), version);
    assert!(result.errors.is_empty(), "{:?}\n{source}", result.errors.first());
    (result.nesting_depth, block_nesting_depth(&result.chunk.block))
}

#[test]
fn counts_the_depth_of_each_construct_as_the_tree_measures_it() {
    for (version, source) in [
        (LuaVersion::Lua54, "return ((a + b) * -c)^2 .. d"),
        (LuaVersion::Lua54, "return a.b[c](d):e{f}\"g\""),
        (LuaVersion::Lua54, "return f{g{h}}, f({g({h})})"),
        (LuaVersion::Lua54, "return {a = {b = {[c] = {d}}}}"),
        (LuaVersion::Lua54, "local function f(...) return function(...) return ... end end"),
        (LuaVersion::Lua54, "a.b.c[d.e].f = g; h(i).j = k"),
        (LuaVersion::Lua54, "for i = a.b, c(d), -e do while f do repeat until g end end"),
        (LuaVersion::Lua54, "if a then elseif b.c then if d then end else do end end"),
        (LuaVersion::Luau, "return `a{b}c{`d{e + f}`}g`, `plain`"),
        (LuaVersion::Luau, "return if a then b else if c then d else e"),
        (LuaVersion::Luau, "return (a :: T) :: U, b :: {V}, f<<T, {U}>>(c), d:m<<T>>()"),
        (LuaVersion::Luau, "type A = {B}? | (C & D) | typeof(e.f) | M.N<O, P<Q>>"),
        (LuaVersion::Luau, "type A = | B | {C}?? type D = & E & (F) -> (G, ...H)"),
        (LuaVersion::Luau, "type A<T = {U}, V... = (W, X)> = <Y>(z: Y, ...{Y}) -> ...Y"),
        (LuaVersion::Luau, "local function f<T>(a: {T}, ...: T?): (T?, ...{T}) end"),
        (LuaVersion::Luau, "local function g(): ({T} | V, ...U) end"),
        (LuaVersion::Luau, "@native @[deprecated{use = 'a'}] local function f() end"),
        (LuaVersion::Luau, "return @[deprecated({reason = 'b'})] function() end"),
        (LuaVersion::Luau, "type function f(a: {b}) return a end"),
        (LuaVersion::Luau, "for i: {T}, v: T? in a do x += f(y) end"),
    ] {
        let (counted, built) = counted_and_built(source, version);
        assert_eq!(counted, built, "{source}");
    }
}

#[test]
fn a_suffix_folded_into_a_return_pack_counts_from_the_whole_pack() {
    // `(T, ...{U})?` parses as `(T?, ...{U})`, and by then only the
    // pack's depth is known, not its first element's.
    let folded = "local function f(): (T, ...{U})? end";
    let written = "local function f(): (T?, ...{U}) end";
    let (counted, built) = counted_and_built(folded, LuaVersion::Luau);
    assert_eq!((counted, built), (6, 5));
    assert_eq!(counted_and_built(written, LuaVersion::Luau), (5, 5));
}

#[test]
fn counts_the_depth_of_generated_programs_as_the_tree_measures_it() {
    for version in LuaVersion::ALL {
        for seed in 0..40 {
            for source in
                [generate_full_grammar(seed, version, 25), generate_runtime_safe(seed, version, 25)]
            {
                let (counted, built) = counted_and_built(&source, version);
                assert_eq!(counted, built, "seed {seed} {version:?}\n{source}");
            }
        }
    }
}

#[test]
fn counts_the_depth_of_fixtures_as_the_tree_measures_it() {
    let mut checked = 0;
    for fixture in lua_fixtures() {
        if fixture.is_error {
            continue;
        }
        let (counted, built) = counted_and_built(&fixture.source, fixture.version);
        assert_eq!(counted, built, "{}", fixture.path.display());
        checked += 1;
    }
    assert!(checked > 0, "no fixture outside `errors/`");
}
