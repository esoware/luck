//! Input nested as deep as Lua 5.4 and Luau take it, and runs and sibling
//! lists far longer, which every stage has to get through on the smallest
//! stack luck gets embedded on.

use core::fmt::Write as _;
use std::thread;

use luck_token::LuaVersion;

/// What a Windows main thread and a wasm module get by default, half of a
/// spawned Rust thread's.
pub const SMALLEST_STACK_BYTES: usize = 1024 * 1024;

fn wrapped(open: &str, count: usize, innermost: &str, close: &str) -> String {
    format!("{}{innermost}{}", open.repeat(count), close.repeat(count))
}

/// One source per construct and dialect: nested to the parser's Lua 5.4
/// limit, and as deep as Luau's own parser gets on a 1 MiB stack, which is
/// its limit or where that stack runs out.
#[must_use]
pub fn sources_at_the_nesting_cap() -> Vec<(LuaVersion, String)> {
    // Before the nesting, what each level opens and closes, the innermost
    // text, and how many levels Lua 5.4 and Luau take.
    let both = [
        ("return ", "(", "x", ")", 198, 998),
        ("return ", "f(", "x", ")", 198, 998),
        ("return ", "a:m(", "x", ")", 198, 831),
        ("return ", "a[", "x", "]", 198, 998),
        ("return ", "{", "1", "}", 198, 998),
        ("return ", "{a=", "1", "}", 198, 998),
        ("return ", "{[1]=", "1", "}", 198, 998),
        ("return ", "f{", "", "}", 199, 832),
        ("return ", "f{{", "", "}}", 99, 499),
        ("return ", "not ", "x", "", 198, 998),
        ("return ", "(", "x", "+x)", 197, 997),
        ("return ", "x..(", "x", ")", 99, 499),
        ("return ", "x^-", "x", "", 99, 499),
        ("return ", "(", "a", ").b", 198, 998),
        ("return ", "(", "f", ")()", 198, 998),
        ("return ", "function() return ", "1", " end", 99, 499),
        ("", "x = function() ", "x = 1", " end", 99, 499),
        ("", "local function f() ", "local x = 1", " end", 198, 750),
        ("", "function f() ", "", " end", 200, 1000),
        ("", "f(function() ", "", " end)", 100, 500),
        ("", "x:m(function() ", "", " end)", 100, 478),
        ("", "f{function() ", "", " end}", 100, 478),
        ("", "do ", "x = 1", " end", 198, 998),
        ("", "if a then ", "x = 1", " end", 198, 998),
        ("", "if a then else ", "x = 1", " end", 198, 998),
        ("", "if a then elseif b then ", "x = 1", " end", 198, 499),
        ("", "while a do ", "x = 1", " end", 198, 998),
        ("", "for i = 1, 2 do ", "x = 1", " end", 198, 998),
        ("", "for k, v in p do ", "x = 1", " end", 198, 998),
        ("", "repeat ", "x = 1", " until a", 198, 998),
    ];
    let luau = [
        ("", "f(function(): T ", "", " end)", 499),
        ("return ", "f(", "x :: T", ")", 997),
        ("return ", "if a then b else ", "x", "", 998),
        ("return ", "`{", "x", "}`", 998),
        ("return ", "(", "x", " :: T)", 997),
        ("type X = ", "{", "T", "}", 499),
        ("type X = ", "(", "T", ")", 499),
        ("type X = ", "A<", "T", " >", 998),
        ("type X = ", "() -> ", "T", "", 498),
        ("type X = ", "(A | ", "T", ")", 332),
        ("type X = ", "typeof(x :: ", "x", ")", 499),
        ("type X = ", "typeof(", "x", ")", 998),
        ("local x: ", "{", "T", "} = 1", 499),
    ];
    let source = |head: &str, open: &str, count, innermost: &str, close: &str| {
        format!("{head}{}", wrapped(open, count, innermost, close))
    };
    both.into_iter()
        .flat_map(|(head, open, innermost, close, lua_count, luau_count)| {
            [
                (LuaVersion::Lua54, source(head, open, lua_count, innermost, close)),
                (LuaVersion::Luau, source(head, open, luau_count, innermost, close)),
            ]
        })
        .chain(luau.into_iter().map(|(head, open, innermost, close, count)| {
            (LuaVersion::Luau, source(head, open, count, innermost, close))
        }))
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
    let luau = [
        format!("return f{}", "<<T>>".repeat(LINKS)),
        format!("a{} += 1", ".b".repeat(LINKS)),
        format!("type X = T{}", "?".repeat(LINKS)),
    ];
    lua.into_iter()
        .map(|source| (LuaVersion::Lua54, source))
        .chain(luau.into_iter().map(|source| (LuaVersion::Luau, source)))
        .collect()
}

/// One source per kind of sibling list, each one level of nesting however
/// long.
fn long_lists() -> Vec<(LuaVersion, String)> {
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

/// One source per construct, each nested far past the cap, where the parser
/// stops.
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
        "\
continue
x()
"
        .repeat(LEVELS),
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
#[expect(
    clippy::print_stderr,
    reason = "the harness shows it only for a test that failed, and an overflow ends the process before the stage can say which source it was on"
)]
pub fn on_the_smallest_stack<Sources>(sources: Sources, stage: fn(&str, LuaVersion))
where
    Sources: IntoIterator<Item = (LuaVersion, String)>,
{
    for (version, source) in sources {
        let dialect = if version.is_luau() { "Luau" } else { "Lua" };
        let head: String = source.chars().take(60).collect();
        eprintln!("{dialect}: {head}");
        #[expect(clippy::expect_used, reason = "the stage can't run without its thread")]
        let worker = thread::Builder::new()
            .stack_size(SMALLEST_STACK_BYTES)
            .spawn(move || stage(&source, version))
            .expect("the thread starts");
        assert!(worker.join().is_ok(), "the stage panicked");
    }
}
