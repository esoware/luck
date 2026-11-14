//! Input nested to the parser's cap, and runs and sibling lists far longer
//! than it, which every stage has to get through on the smallest stack luck
//! gets embedded on, in an unoptimized build too.

use core::fmt::Write as _;
use std::thread;

use luck_token::LuaVersion;

/// What a Windows main thread and a wasm module get by default, half of a
/// spawned Rust thread's.
pub const SMALLEST_STACK_BYTES: usize = 1024 * 1024;

fn wrapped(open: &str, count: usize, innermost: &str, close: &str) -> String {
    format!("{}{innermost}{}", open.repeat(count), close.repeat(count))
}

/// One source per construct, each nested as deep as the parser accepts.
#[must_use]
pub fn sources_at_the_nesting_cap() -> Vec<(LuaVersion, String)> {
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
#[must_use]
pub fn long_runs() -> Vec<(LuaVersion, String)> {
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

/// One source per kind of sibling list, each one level of nesting however
/// long.
#[must_use]
pub fn long_lists() -> Vec<(LuaVersion, String)> {
    const ITEMS: usize = 20_000;
    let numbered = |template: &str, separator: &str| {
        (0..ITEMS)
            .map(|i| template.replace('#', &i.to_string()))
            .collect::<Vec<_>>()
            .join(separator)
    };
    let falsy_arms = " elseif false then x()".repeat(ITEMS);
    let live_arms = " elseif a then x()".repeat(ITEMS);
    let names = numbered("a#", ",");
    let mut dead_chain = String::from("local function f0() end\n");
    for (i, previous) in (1..ITEMS).zip(0usize..) {
        let _unused = writeln!(dead_chain, "local function f{i}() f{previous}() end");
    }
    let lua = [
        "x() ".repeat(ITEMS),
        "x() -- c\n".repeat(ITEMS),
        dead_chain,
        format!(
            "local function f() local {names} = {} return {names} end return f",
            numbered("#", ",")
        ),
        format!(
            "local function f() local {names} {} return {names} end return f",
            numbered("a# = #", " ")
        ),
        format!("if false then x(){falsy_arms} end"),
        format!("if a then x(){live_arms} end"),
        format!("if a then x(){falsy_arms} end"),
        format!(
            "if false then x(){falsy_arms} elseif a then x() elseif true then x(){live_arms} else x() end"
        ),
        format!("if a then x(){live_arms} elseif 1 then x(){live_arms} else x() end"),
        format!("return {{{}}}", "x,".repeat(ITEMS)),
        format!("return {{{}}}", numbered("a# = x", ",")),
        format!("return f(x{})", ",x".repeat(ITEMS)),
        format!("return x{}", ",x".repeat(ITEMS)),
        format!("x{} = 1", ",x".repeat(ITEMS)),
        format!("local {} = 1 return a0", numbered("a#", ",")),
        format!("local function f({}) return a0 end return f", numbered("a#", ",")),
        format!("local function f() {} return a0 end return f", numbered("local a# = #", " ")),
        format!("local function f() {} return a0 end return f", numbered("a# = #", " ")),
        format!("{}::l::", "goto l ".repeat(ITEMS)),
        numbered("::l#::", " "),
    ];
    let luau = [
        format!("return if a then b{} else c", " elseif a then b".repeat(ITEMS)),
        format!("return if false then b{} else c", " elseif false then b".repeat(ITEMS)),
        format!("return `{}`", "{x}".repeat(ITEMS)),
        format!("type X = A{}", " | A".repeat(ITEMS)),
        format!("type X = A{}", " & A".repeat(ITEMS)),
        format!("type X = {{{}}}", numbered("a#: T", ",")),
        format!("type X = (T{}) -> T", ",T".repeat(ITEMS)),
        format!("type X<{}> = T0", numbered("T#", ",")),
    ];
    lua.into_iter()
        .map(|source| (LuaVersion::Lua54, source))
        .chain(luau.into_iter().map(|source| (LuaVersion::Luau, source)))
        .collect()
}

/// One source per construct, each nested far past the cap, which the parser
/// skips without recursing.
#[must_use]
pub fn sources_past_the_nesting_cap() -> Vec<(LuaVersion, String)> {
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

/// One source per way of breaking input at every level, far past the cap,
/// which parse recovery reads in loops, in time linear in its length.
#[must_use]
pub fn broken_sources() -> Vec<(LuaVersion, String)> {
    const LEVELS: usize = 100_000;
    let lua = [
        format!("return {}x", "(".repeat(LEVELS)),
        format!("return {}x{}", "(".repeat(LEVELS), "]".repeat(LEVELS)),
        format!("return {}x{}", "{".repeat(LEVELS), "end ".repeat(LEVELS)),
        format!("return {}{}", "(".repeat(LEVELS), "do )".repeat(LEVELS)),
        format!("return {}", "f(function() ".repeat(LEVELS)),
        "do ".repeat(LEVELS),
        "if a then f( ".repeat(LEVELS),
        "while a(b do ".repeat(LEVELS),
        "end ".repeat(LEVELS),
        ") ] } ".repeat(LEVELS),
        "x = = 1\n".repeat(LEVELS),
        "return 1\n".repeat(LEVELS),
        "f(\"a\n".repeat(LEVELS),
        "x = 'a\\q' .. '\\x' y\n".repeat(LEVELS),
    ];
    let luau = [
        format!("return {}x", "`{".repeat(LEVELS)),
        format!("return {}x{}", "`{".repeat(LEVELS), ")".repeat(LEVELS)),
        "local x = `{y}a\n".repeat(LEVELS),
        "local x: A<B = 1\n".repeat(LEVELS),
        "continue\nx()\n".repeat(LEVELS),
    ];
    lua.into_iter()
        .map(|source| (LuaVersion::Lua54, source))
        .chain(luau.into_iter().map(|source| (LuaVersion::Luau, source)))
        .collect()
}

/// Every source at the cap, every long run, and every long list.
#[must_use]
pub fn deep_and_long_sources() -> Vec<(LuaVersion, String)> {
    let mut sources = sources_at_the_nesting_cap();
    sources.extend(long_runs());
    sources.extend(long_lists());
    sources
}

/// Runs `stage` over every source, each on a thread with
/// [`SMALLEST_STACK_BYTES`] of stack. Overflowing it takes the whole test
/// process down.
///
/// # Panics
///
/// Panics when a thread can't start, or when `stage` panics.
pub fn on_the_smallest_stack<Sources>(sources: Sources, stage: fn(&str, LuaVersion))
where
    Sources: IntoIterator<Item = (LuaVersion, String)>,
{
    for (version, source) in sources {
        #[expect(clippy::expect_used, reason = "the stage can't run without its thread")]
        let worker = thread::Builder::new()
            .stack_size(SMALLEST_STACK_BYTES)
            .spawn(move || stage(&source, version))
            .expect("the thread starts");
        assert!(worker.join().is_ok(), "the stage panicked");
    }
}
