//! Input nested to the parser's cap, and runs and sibling lists far longer
//! than it, which every stage has to get through on
//! [`luck_ast::MIN_STACK_BYTES`].

use std::fmt::Write as _;
use std::{panic, thread};

use luck_ast::{MAX_NESTING_DEPTH, MIN_STACK_BYTES};

const LEVELS: usize = 100_000;

/// `innermost` inside `count` of `open`, each closed by a `close`.
pub fn nested(open: &str, count: usize, innermost: &str, close: &str) -> String {
    format!("{}{innermost}{}", open.repeat(count), close.repeat(count))
}

/// One source per construct, each nested as deep as the parser accepts.
pub fn sources_at_the_nesting_cap() -> Vec<String> {
    // How many times a construct that nests one, two, three, or four levels
    // fits under the chunk's own. Parentheses the tree leaves out open none,
    // and as many as the cap may be open at once over a tree at it.
    let one = MAX_NESTING_DEPTH - 1;
    let (two, three, four) = (one / 2, one / 3, one / 4);
    let groups = MAX_NESTING_DEPTH;
    vec![
        format!("return {}", nested("(", groups, "x", ")")),
        format!("return {}", nested("(", groups, &nested("{", one, "1", "}"), ")")),
        format!("return {}", nested("f(", one, "x", ")")),
        format!("return {}", nested("a:m(", one, "x", ")")),
        format!("return {}", nested("a[", one, "x", "]")),
        format!("return {}", nested("{", one, "1", "}")),
        format!("return {}", nested("{a=", one, "1", "}")),
        format!("return {}", nested("{[1]=", one, "1", "}")),
        format!("return {}", nested("f{", two, "", "}")),
        format!("return {}x", "not ".repeat(one)),
        format!("return {}x{}", "(".repeat(one), "+x)".repeat(one)),
        format!("return {}", nested("x..(", one, "x", ")")),
        format!("return {}", nested("x^-", two, "x", "")),
        format!("return {}a{}", "(".repeat(groups), ").b".repeat(groups)),
        format!("return {}f{}", "(".repeat(two), ")()".repeat(two)),
        format!("return {}", nested("function() return ", two, "1", " end")),
        nested("x = function() ", two, "x = 1", " end"),
        nested("local function f() ", two, "local x = 1", " end"),
        nested("f(function() ", three, "x = 1", " end)"),
        nested("a:m(function() ", three, "x = 1", " end)"),
        nested("f{function() ", four, "x = 1", " end}"),
        nested("(function() ", three, "x = 1", " end)()"),
        nested("do ", one, "x = 1", " end"),
        nested("if a then ", one, "x = 1", " end"),
        nested("if a then else ", one, "x = 1", " end"),
        nested("if a then elseif b then ", one, "x = 1", " end"),
        nested("while a do ", one, "x = 1", " end"),
        nested("for i = 1, 2 do ", one, "x = 1", " end"),
        nested("for k, v in p do ", one, "x = 1", " end"),
        nested("repeat ", one, "x = 1", " until a"),
        format!("return {}x", "if a then b else ".repeat(one)),
        format!("return {}", nested("`{", one, "x", "}`")),
        format!("return {}", nested("(", one, "x", " :: T)")),
        format!("type X = {}", nested("{", one, "T", "}")),
        format!("type X = {}", nested("(", one, "T", ")")),
        format!("type X = {}", nested("A<", one, "T", " >")),
        format!("type X = {}T", "() -> ".repeat(one)),
        format!("type X = {}", nested("(A | ", two, "T", ")")),
        format!("type X = {}", nested("typeof(x :: ", two, "x", ")")),
        format!("local x: {} = 1", nested("{", one, "T", "}")),
    ]
}

/// One source per kind of run, each one level of nesting however long.
fn long_runs() -> Vec<String> {
    const LINKS: usize = 20_000;
    vec![
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
        format!("return f{}", "<<T>>".repeat(LINKS)),
        format!("a{} += 1", ".b".repeat(LINKS)),
        format!("type X = T{}", "?".repeat(LINKS)),
    ]
}

/// One source per kind of sibling list, each one level of nesting however
/// long.
fn long_lists() -> Vec<String> {
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
    for i in 1..ITEMS {
        let _ = writeln!(dead_chain, "local function f{i}() f{}() end", i - 1);
    }
    vec![
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
        format!("return if a then b{} else c", " elseif a then b".repeat(ITEMS)),
        format!("return if false then b{} else c", " elseif false then b".repeat(ITEMS)),
        format!("return `{}`", "{x}".repeat(ITEMS)),
        format!("type X = A{}", " | A".repeat(ITEMS)),
        format!("type X = A{}", " & A".repeat(ITEMS)),
        format!("type X = {{{}}}", numbered("a#: T", ",")),
        format!("type X = (T{}) -> T", ",T".repeat(ITEMS)),
        format!("type X<{}> = T0", numbered("T#", ",")),
    ]
}

/// One source per construct, each nested far past the cap, where the parser
/// stops.
pub fn sources_past_the_nesting_cap() -> Vec<String> {
    vec![
        format!("return {}", nested("(", LEVELS, "x", ")")),
        format!("return {}", nested("f(", LEVELS, "x", ")")),
        format!("return {}", nested("a[", LEVELS, "x", "]")),
        format!("return {}", nested("{", LEVELS, "1", "}")),
        format!("return {}", nested("f{", LEVELS, "", "}")),
        format!("return {}x", "- ".repeat(LEVELS)),
        format!("return {}", nested("x^-", LEVELS, "x", "")),
        format!("return {}", nested("function() return ", LEVELS, "1", " end")),
        nested("do ", LEVELS, "x = 1", " end"),
        nested("if a then ", LEVELS, "x = 1", " end"),
        nested("while a do ", LEVELS, "x = 1", " end"),
        nested("repeat ", LEVELS, "x = 1", " until a"),
        nested("local function f() ", LEVELS, "local x = 1", " end"),
        format!("return {}x", "if a then b else ".repeat(LEVELS)),
        format!("return {}a{}", "if ".repeat(LEVELS), " then b else c".repeat(LEVELS)),
        format!("return {}", nested("`{", LEVELS, "x", "}`")),
        format!("return {}", nested("(", LEVELS, "x", " :: T)")),
        format!("type X = {}", nested("{", LEVELS, "T", "}")),
        format!("type X = {}", nested("A<", LEVELS, "T", ">")),
        format!("type X = {}T", "() -> ".repeat(LEVELS)),
        format!("type X = {}T", "() -> ...".repeat(LEVELS)),
        format!("type X = {}", nested("typeof(", LEVELS, "x", ")")),
    ]
}

/// One source per way of breaking input at every level, far past the cap,
/// which parse recovery reads in loops, in time linear in its length.
pub fn broken_sources() -> Vec<String> {
    vec![
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
        format!("return {}x", "`{".repeat(LEVELS)),
        format!("return {}x{}", "`{".repeat(LEVELS), ")".repeat(LEVELS)),
        format!("return {}1{}", "@[deprecated(".repeat(LEVELS), ")]".repeat(LEVELS)),
        "local x = `{y}a\n".repeat(LEVELS),
        "local x: A<B = 1\n".repeat(LEVELS),
        "\
continue
x()
"
        .repeat(LEVELS),
    ]
}

/// Every source at the cap, every long run, and every long list.
pub fn deep_and_long_sources() -> Vec<String> {
    let mut sources = sources_at_the_nesting_cap();
    sources.extend(long_runs());
    sources.extend(long_lists());
    sources
}

/// Runs `stage` over every source, each [`with_the_smallest_stack`].
///
/// # Panics
///
/// Panics when a thread can't start, or with what `stage` panics with.
pub fn on_the_smallest_stack<Sources>(sources: Sources, stage: fn(&str))
where
    Sources: IntoIterator<Item = String>,
{
    for source in sources {
        with_the_smallest_stack(|| stage(&source));
    }
}

/// Runs `work` on a thread with [`MIN_STACK_BYTES`] of stack, as luck asks of
/// every thread that runs a stage. Overflowing it takes the whole test
/// process down.
///
/// # Panics
///
/// Panics when the thread can't start, or with what `work` panics with.
pub fn with_the_smallest_stack<T, Work>(work: Work) -> T
where
    T: Send,
    Work: FnOnce() -> T + Send,
{
    thread::scope(|scope| {
        let worker = thread::Builder::new()
            .stack_size(MIN_STACK_BYTES)
            .spawn_scoped(scope, work)
            .expect("the thread starts");
        worker.join().unwrap_or_else(|payload| panic::resume_unwind(payload))
    })
}
