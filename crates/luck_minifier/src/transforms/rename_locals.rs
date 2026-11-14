mod analyzer;
mod candidate_pool;
mod globals;
mod renamer;
mod scope_claims;

use luck_ast::{Block, Transform as _, Visitor as _};
use luck_token::LuaVersion;

use self::analyzer::Analyzer;
use self::globals::renamable_globals;
use self::renamer::Renamer;

pub(crate) fn rename(block: Block, version: LuaVersion, rename_globals: bool) -> Block {
    // Opt-in, since renamed globals end up under new `_G` keys.
    let root_globals = if rename_globals { renamable_globals(&block, version) } else { Vec::new() };

    // The analyzer resolves names with the same scoping rules as the renamer,
    // so a reference that resolves to no binding is a global. A flat name set
    // can't tell a global read from a same-named local elsewhere in the file.
    let mut analyzer = Analyzer::new(version);
    analyzer.declare_root_globals(&root_globals);
    analyzer.visit_block(&block);

    let mut renamer = Renamer::new(analyzer.renamed_bindings(), version);
    renamer.declare_root_globals(&root_globals);
    let output = renamer.transform_block(block);
    debug_assert!(
        renamer.has_declared_every_binding(),
        "the renamer's walk declared fewer bindings than its analyzer's"
    );
    output
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        let output = run_pass(source, version, |block| rename(block, version, true));
        assert_reparses(&output, version);
        output
    }

    #[test]
    fn many_locals_live_at_once_rename_without_rescanning_slots_or_names() {
        // Each local needs a slot and a name of its own. Looking either up
        // from the first one every time takes minutes at this size.
        let mut source = String::new();
        for local in 0..120_000u32 {
            let _unused = writeln!(source, "local a{local} = f({local})");
        }
        source.push_str("return a0, a119999");
        let result = apply(&source);
        // The two locals read twice take the two shortest names.
        assert!(
            result.starts_with("local l=f(0)local c=f(1)local k=f(2)"),
            "{:?}",
            result.get(..60)
        );
        assert!(result.ends_with("return l,u"), "the output ends in the return");
    }

    #[test]
    fn many_sibling_scopes_share_one_name() {
        let mut source = String::new();
        for scope in 0..20_000u32 {
            let _unused = writeln!(source, "do local x{scope} = f() g(x{scope}) end");
        }
        let result = apply(&source);
        assert_eq!(result, "do local l=f()g(l)end ".repeat(20_000).trim_end());
    }

    #[test]
    fn renames_locals_to_short_names() {
        let result = apply(
            "\
local longname = 1
return longname
",
        );
        assert!(!result.contains("longname"), "Local should be renamed: {result}");
    }

    #[test]
    fn preserves_globals() {
        let result = apply("print(42)\n");
        assert!(result.contains("print"), "Global should not be renamed: {result}");
    }

    #[test]
    fn renames_function_params() {
        let result = apply(
            "\
local function foo(longparam)
  return longparam
end
return foo
",
        );
        assert!(!result.contains("longparam"), "Param should be renamed: {result}");
    }

    #[test]
    fn preserves_field_names() {
        let result = apply(
            "\
local t = {}
t.field = 1
return t.field
",
        );
        assert!(result.contains("field"), "Field name should not be renamed: {result}");
    }

    #[test]
    fn renames_a_nested_scope_to_valid_lua() {
        apply(
            "\
local x = 1
do
  local y = 2
  print(y)
end
return x
",
        );
    }

    #[test]
    fn most_frequent_local_gets_shortest_name() {
        let result = apply(
            "\
local rare = 1
local frequent = 2
print(frequent)
print(frequent)
print(frequent)
print(rare)
return frequent, rare
",
        );
        assert!(!result.contains("frequent"), "'frequent' should be renamed: {result}");
        assert!(!result.contains("rare"), "'rare' should be renamed: {result}");
    }

    #[test]
    fn preserves_self_parameter() {
        let result = apply(
            "\
local t = {}
function t:method()
  return self
end
return t
",
        );
        assert!(result.contains("self"), "'self' should not be renamed: {result}");
    }

    #[test]
    fn sibling_scopes_reuse_names() {
        let result = apply(
            "\
do
  local longvar = 1
  print(longvar)
end
do
  local another = 2
  print(another)
end
",
        );
        assert!(
            !result.contains("longvar") && !result.contains("another"),
            "Both should be renamed: {result}"
        );
    }

    #[test]
    fn renames_nested_function_locals_to_valid_lua() {
        apply(
            "\
local outer = 1
local function f()
  local inner = 2
  print(inner, inner, inner, inner, inner)
end
return outer, f
",
        );
    }

    #[test]
    fn locals_can_shadow_unused_globals() {
        // 'l' and 'u' are globals at top level but not inside f.
        let result = apply(
            "\
l(1)
u(2)
local function f()
  local longvar1 = 3
  local longvar2 = 4
  return longvar1 + longvar2
end
return f
",
        );
        assert!(!result.contains("longvar"), "locals should be renamed: {result}");
    }

    #[test]
    fn locals_avoid_globals_used_in_same_scope() {
        // 'l' is a global called inside f, so locals must not shadow it.
        let result = apply(
            "\
local function f()
  local longvar = 1
  l(longvar)
  return longvar
end
return f
",
        );
        assert!(result.contains("l("), "global 'l' must be preserved: {result}");
        assert!(!result.contains("longvar"), "longvar should be renamed: {result}");
    }

    #[test]
    fn renames_inside_global_function_body() {
        // The analyzer and the renamer must both walk `global function` bodies,
        // or the binding ids fall out of step.
        let result = apply_version(
            "\
global function f(longparam)
  return longparam
end
",
            LuaVersion::Lua55,
        );
        assert!(
            !result.contains("longparam"),
            "param inside global function should be renamed: {result}"
        );
        assert!(result.contains("function f("), "global function name must be preserved: {result}");
    }

    fn apply_luau(source: &str) -> String {
        apply_version(source, LuaVersion::Luau)
    }

    #[test]
    fn global_read_via_typecast_is_not_assign_first() {
        // `g` is read inside a type cast before it's assigned, so it's a real
        // global and must keep its name.
        let result = apply_luau(
            "\
h = (g :: any)
g = 1
print(h, g)
",
        );
        assert!(result.contains('g'), "global 'g' read via typecast must be preserved: {result}");
    }

    #[test]
    fn global_read_via_compound_index_is_not_assign_first() {
        // `g` is read as the index of a compound target before it's assigned,
        // so it's a real global and must keep its name, at any nesting depth.
        for (source, expected) in [
            (
                "\
t[g] += 1
g = 1
print(t, g)
",
                "t[g]+=1 g=1 print(t,g)",
            ),
            (
                "\
do t[g] += 1 end
g = 1
print(t, g)
",
                "do t[g]+=1 end g=1 print(t,g)",
            ),
        ] {
            let result = apply_luau(source);
            assert_eq!(result, expected);
        }
    }

    fn assert_luau_renames(cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            let result = apply_luau(source);
            assert_eq!(result, expected, "source: {source}");
        }
    }

    #[test]
    fn global_read_via_assignment_target_is_not_assign_first() {
        // A target's prefix and index are reads at any depth of the target
        // chain, so `g` is a real global and must keep its name.
        assert_luau_renames(&[
            (
                "\
t[g] = 1
g = 1
print(t, g)
",
                "t[g]=1 g=1 print(t,g)",
            ),
            (
                "\
g[1] = 2
g = 1
print(g)
",
                "g[1]=2 g=1 print(g)",
            ),
            (
                "\
g.a.b = 1
g = 1
print(g)
",
                "g.a.b=1 g=1 print(g)",
            ),
            (
                "\
g[1].b = 1
g = 1
print(g)
",
                "g[1].b=1 g=1 print(g)",
            ),
            (
                "\
t[g].b = 1
g = 1
print(t, g)
",
                "t[g].b=1 g=1 print(t,g)",
            ),
            (
                "\
f(g).x = 1
g = 1
print(g)
",
                "f(g).x=1 g=1 print(g)",
            ),
            (
                "\
(g).x = 1
g = 1
print(g)
",
                "(g).x=1 g=1 print(g)",
            ),
            (
                "\
t[g], g = 1, 2
print(t, g)
",
                "t[g],g=1,2 print(t,g)",
            ),
        ]);
    }

    #[test]
    fn global_read_by_later_target_is_not_assign_first() {
        // Every target's prefix and index is evaluated before any store, so `g`
        // is read here even though its own target comes first.
        assert_luau_renames(&[
            (
                "\
g, t[g] = 1, 2
print(t, g)
",
                "g,t[g]=1,2 print(t,g)",
            ),
            (
                "\
g, g[1] = 1, 2
print(g)
",
                "g,g[1]=1,2 print(g)",
            ),
            (
                "\
g, g.x = 1, 2
print(g)
",
                "g,g.x=1,2 print(g)",
            ),
        ]);
    }

    #[test]
    fn global_read_via_nested_return_is_not_assign_first() {
        // A return sits outside its block's statement list, and an early one
        // reads `g` before the assignment that follows the block.
        assert_luau_renames(&[
            (
                "\
do return g end
g = 1
print(g)
",
                "do return g end g=1 print(g)",
            ),
            (
                "\
if c then return g end
g = 1
print(g)
",
                "if c then return g end g=1 print(g)",
            ),
            (
                "\
if c then print(1) elseif d then return g end
g = 1
print(g)
",
                "if c then print(1)elseif d then return g end g=1 print(g)",
            ),
            (
                "\
if c then print(1) else return g end
g = 1
print(g)
",
                "if c then print(1)else return g end g=1 print(g)",
            ),
            (
                "\
while c do return g end
g = 1
print(g)
",
                "while c do return g end g=1 print(g)",
            ),
            (
                "\
repeat return g until c
g = 1
print(g)
",
                "repeat return g until c g=1 print(g)",
            ),
            (
                "\
for i = 1, 2 do return g end
g = 1
print(g)
",
                "for l=1,2 do return g end g=1 print(g)",
            ),
            (
                "\
for k in x do return g end
g = 1
print(g)
",
                "for l in x do return g end g=1 print(g)",
            ),
        ]);
    }

    #[test]
    fn global_read_via_function_name_is_not_assign_first() {
        // `function g.f()` and `function g:f()` read `g` to store into it.
        assert_luau_renames(&[
            (
                "\
function g.x() end
g = 1
print(g)
",
                "function g.x()end g=1 print(g)",
            ),
            (
                "\
function g:m() end
g = 1
print(g)
",
                "function g:m()end g=1 print(g)",
            ),
            (
                "\
function g.a.b() end
g = 1
print(g)
",
                "function g.a.b()end g=1 print(g)",
            ),
            (
                "\
do function g.x() end end
g = 1
print(g)
",
                "do function g.x()end end g=1 print(g)",
            ),
        ]);
    }

    #[test]
    fn global_assigned_before_any_read_stays_assign_first() {
        // Later reads, a plain `function g()` (which only assigns), and a read
        // deferred into a function body all leave `g` assign-first, so it's
        // still renamed. In the last case, `h` stays assign-first even though
        // a target beside it reads `g`.
        assert_luau_renames(&[
            (
                "\
g = 1
t[g] = 2
print(t, g)
",
                "l=1 t[l]=2 print(t,l)",
            ),
            (
                "\
g = 1
g.a.b = 2
print(g)
",
                "l=1 l.a.b=2 print(l)",
            ),
            (
                "\
g = 1
function g.x() end
print(g)
",
                "l=1 function l.x()end print(l)",
            ),
            (
                "\
do function g() end end
g = 1
print(g)
",
                "do function l()end end l=1 print(l)",
            ),
            (
                "\
h = function() return g end
g = 1
print(h, g)
",
                "u=function()return l end l=1 print(u,l)",
            ),
            (
                "\
h, t[g] = 1, 2
print(h, t)
",
                "l,t[g]=1,2 print(l,t)",
            ),
        ]);
    }

    // Same reads as the assign-first check, at the same depths.
    const READ_BEFORE_DEFINITION_CASES: &[(&str, &str)] = &[
        (
            "\
print(g)
function g() end
",
            "print(g)function g()end",
        ),
        (
            "\
x = g
function g() end
",
            "l=g function g()end",
        ),
        (
            "\
t[g] = 1
function g() end
",
            "t[g]=1 function g()end",
        ),
        (
            "\
g.x = 1
function g() end
",
            "g.x=1 function g()end",
        ),
        (
            "\
g[1] = 1
function g() end
",
            "g[1]=1 function g()end",
        ),
        (
            "\
g, t[g] = 1, 2
function g() end
",
            "g,t[g]=1,2 function g()end",
        ),
        (
            "\
g += 1
function g() end
",
            "g+=1 function g()end",
        ),
        (
            "\
function g.x() end
function g() end
",
            "function g.x()end function g()end",
        ),
        (
            "\
function g:m() end
function g() end
",
            "function g:m()end function g()end",
        ),
        (
            "\
if c then return g end
function g() end
",
            "if c then return g end function g()end",
        ),
        (
            "\
do print(g) end
function g() end
",
            "do print(g)end function g()end",
        ),
        (
            "\
print(g)
function g() end
function g() end
",
            "print(g)function g()end function g()end",
        ),
        (
            "\
print(g)
function g() end
g = 1
print(g)
",
            "print(g)function g()end g=1 print(g)",
        ),
        (
            "\
print(g)
do local g = 1 end
function g() end
",
            "print(g)do local l=1 end function g()end",
        ),
    ];

    #[test]
    fn global_read_before_function_definition_is_not_renamed() {
        assert_luau_renames(READ_BEFORE_DEFINITION_CASES);
    }

    #[test]
    fn nested_read_before_the_local_it_shares_a_name_with_is_a_global_read() {
        assert_luau_renames(&[
            (
                "\
do print(g)
local g = 1 end
function g() end
",
                "do print(g)local l=1 end function g()end",
            ),
            (
                "\
do local g = g end
function g() end
",
                "do local l=g end function g()end",
            ),
            (
                "\
do do local g = f() end
print(g) end
function g() end
",
                "do do local l=f()end print(g)end function g()end",
            ),
            (
                "\
for g = g, 2 do end
function g() end
",
                "for l=g,2 do end function g()end",
            ),
            (
                "\
for g in g do end
function g() end
",
                "for l in g do end function g()end",
            ),
            (
                "\
repeat print(g)
local g = f() until g
function g() end
",
                "repeat print(g)local l=f()until l function g()end",
            ),
        ]);
    }

    #[test]
    fn read_of_a_nested_local_is_not_a_read_of_the_global() {
        assert_luau_renames(&[
            (
                "\
do local g = f()
print(g) end
function g() end
",
                "do local l=f()print(l)end function l()end",
            ),
            (
                "\
do local function g() end
g() end
function g() end
",
                "do local function l()end l()end function l()end",
            ),
            (
                "\
for g = 1, 2 do print(g) end
function g() end
",
                "for l=1,2 do print(l)end function l()end",
            ),
            (
                "\
for g in x do print(g) end
function g() end
",
                "for l in x do print(l)end function l()end",
            ),
            (
                "\
repeat local g = f() until g
function g() end
",
                "repeat local l=f()until l function l()end",
            ),
        ]);
    }

    #[test]
    fn assignment_before_any_read_keeps_a_function_global() {
        // The tie between `a` and `g` goes to the earlier binding, so moving
        // `g` out of the function globals would swap their names.
        assert_luau_renames(&[
            (
                "\
g = 1
do local g = f() end
print(g)
function g() end
",
                "l=1 do local l=f()end print(l)function l()end",
            ),
            (
                "\
a = 1
print(a)
print(a)
g = 1
print(g)
function g() end
",
                "u=1 print(u)print(u)l=1 print(l)function l()end",
            ),
        ]);
    }

    #[test]
    fn function_defined_before_any_read_stays_renamed() {
        // A read after the definition and a function body's deferred read both
        // leave `g` renamed, and so does an assignment that got there first.
        assert_luau_renames(&[
            (
                "\
function g() end
print(g)
",
                "function l()end print(l)",
            ),
            ("function g() return g end\n", "function l()return l end"),
            (
                "\
function g() end
print(g)
function g() end
",
                "function l()end print(l)function l()end",
            ),
            (
                "\
h = function() return g end
function g() end
",
                "u=function()return l end function l()end",
            ),
            (
                "\
function f() return g end
f()
function g() end
",
                "function l()return u end l()function u()end",
            ),
            (
                "\
g = 1
function g() end
print(g)
",
                "l=1 function l()end print(l)",
            ),
            (
                "\
g = 1
print(g)
function g() end
",
                "l=1 print(l)function l()end",
            ),
            (
                "\
do local g = 1 end
function g() end
",
                "do local l=1 end function l()end",
            ),
        ]);
    }

    #[test]
    fn deferred_global_read_is_left_as_it_was() {
        let result = apply(
            "\
function f() return g end
f()
g = 1
",
        );
        assert_eq!(result, "function l()return u end l()u=1");
    }

    const ENV_VERSIONS: [LuaVersion; 4] =
        [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55];

    #[test]
    fn env_is_never_renamed_on_versions_with_an_env_upvalue() {
        for version in ENV_VERSIONS {
            for (source, expected) in [
                (
                    "\
_ENV = {}
print(1)
",
                    "_ENV={}print(1)",
                ),
                (
                    "\
_ENV = {}
_ENV = {}
",
                    "_ENV={}_ENV={}",
                ),
                (
                    "\
_ENV, x = {}, 1
print(x)
",
                    "_ENV,l={},1 print(l)",
                ),
                ("function _ENV() end\n", "function _ENV()end"),
                ("function _ENV.f() end\n", "function _ENV.f()end"),
                (
                    "\
local _ENV = {}
print(1)
",
                    "local _ENV={}print(1)",
                ),
                (
                    "\
local _ENV = {}
_ENV = {}
",
                    "local _ENV={}_ENV={}",
                ),
                (
                    "\
do local _ENV = {}
_ENV = {} end
_ENV = {}
",
                    "do local _ENV={}_ENV={}end _ENV={}",
                ),
                ("local function _ENV() end\n", "local function _ENV()end"),
                (
                    "\
local function f(_ENV) return x end
return f
",
                    "local function l(_ENV)return x end return l",
                ),
                ("for _ENV = 1, 2 do print(x) end\n", "for _ENV=1,2 do print(x)end"),
                (
                    "for k, _ENV in pairs(t) do print(k) end\n",
                    "for l,_ENV in pairs(t)do print(l)end",
                ),
            ] {
                let result = apply_version(source, version);
                assert_eq!(result, expected, "{version:?} source: {source}");
            }
        }
    }

    #[test]
    fn env_is_an_ordinary_name_where_there_is_no_env_upvalue() {
        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            for (source, expected) in [
                (
                    "\
_ENV = 1
print(_ENV)
",
                    "l=1 print(l)",
                ),
                (
                    "\
function _ENV() end
print(_ENV)
",
                    "function l()end print(l)",
                ),
                (
                    "\
local _ENV = 1
print(_ENV)
",
                    "local l=1 print(l)",
                ),
                ("for _ENV = 1, 2 do print(_ENV) end\n", "for l=1,2 do print(l)end"),
                (
                    "for k, _ENV in pairs(t) do print(_ENV) end\n",
                    "for u,l in pairs(t)do print(l)end",
                ),
            ] {
                let result = apply_version(source, version);
                assert_eq!(result, expected, "{version:?} source: {source}");
            }
        }
    }

    fn assert_lua55_renames(cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            let result = apply_version(source, LuaVersion::Lua55);
            assert_eq!(result, expected, "source: {source}");
        }
    }

    #[test]
    fn globally_declared_name_is_not_renamed() {
        assert_lua55_renames(&[
            (
                "\
global g
g = 1
",
                "global g g=1",
            ),
            (
                "\
g = 1
global g
",
                "g=1 global g",
            ),
            (
                "\
g = 1
global g
g = 2
",
                "g=1 global g g=2",
            ),
            (
                "\
global g = 1
g = 2
",
                "global g=1 g=2",
            ),
            (
                "\
global a, g
g = 1
a = 2
",
                "global a,g g=1 a=2",
            ),
            ("global <const> g = 1\n", "global g<const> =1"),
            (
                "\
function g() end
global g
",
                "function g()end global g",
            ),
            (
                "\
global g
function g() end
",
                "global g function g()end",
            ),
        ]);
    }

    #[test]
    fn name_of_global_function_is_not_renamed() {
        assert_lua55_renames(&[
            (
                "\
g = 1
global function g() end
",
                "g=1 global function g()end",
            ),
            (
                "\
function g() end
global function g() end
",
                "function g()end global function g()end",
            ),
            (
                "\
global function g() return g end
g()
",
                "global function g()return g end g()",
            ),
        ]);
    }

    #[test]
    fn global_declaration_pins_its_name_from_any_depth() {
        assert_lua55_renames(&[
            (
                "\
g = 1
do global g
g = 2 end
",
                "g=1 do global g g=2 end",
            ),
            (
                "\
g = 1
if c then global g end
",
                "g=1 if c then global g end",
            ),
            (
                "\
g = 1
function f() global g
g = 2 end
",
                "g=1 function l()global g g=2 end",
            ),
            (
                "\
function g() end
local function f() global function g() end end
return f
",
                "function g()end local function l()global function g()end end return l",
            ),
        ]);
    }

    #[test]
    fn global_declaration_shadows_an_earlier_local() {
        // After the declaration `g` is the global, and until then it's the
        // local, so only the uses before it may take the local's new name.
        assert_lua55_renames(&[
            (
                "\
local g = f()
print(g)
global g, print
g = 2
print(g)
",
                "local l=f()print(l)global g,print g=2 print(g)",
            ),
            (
                "\
local g = f()
do global g
g = 2 end
print(g)
",
                "local l=f()do global g g=2 end print(l)",
            ),
            (
                "\
local g = f()
global function g() return g end
",
                "local l=f()global function g()return g end",
            ),
        ]);
    }

    #[test]
    fn global_declaration_takes_effect_after_its_initializers() {
        assert_lua55_renames(&[(
            "\
local g = f()
global g = g
g = 2
",
            "local l=f()global g=l g=2",
        )]);
    }

    #[test]
    fn local_declared_after_a_global_declaration_is_still_renamed() {
        assert_lua55_renames(&[(
            "\
global g, print
do
local g = f()
print(g)
end
g = 1
",
            "global g,print do local l=f()print(l)end g=1",
        )]);
    }

    #[test]
    fn renamed_local_never_wears_a_globally_declared_name() {
        // Lookup finds the newest declaration, so a local wearing `l` here
        // would read the global `l` as soon as the declaration is in scope,
        // whether or not anything else reads that global.
        assert_lua55_renames(&[
            (
                "\
local longname = f()
global l, print
print(longname)
",
                "local u=f()global l,print print(u)",
            ),
            (
                "\
local longname = f()
do global l, print
print(longname) end
",
                "local u=f()do global l,print print(u)end",
            ),
            (
                "\
global l, print, f
do
local longname = f()
print(longname, l)
end
",
                "global l,print,f do local u=f()print(u,l)end",
            ),
            (
                "\
local longname = f()
global function l() end
return longname
",
                "local u=f()global function l()end return u",
            ),
        ]);
    }

    #[test]
    fn collective_global_declaration_leaves_every_name_free() {
        // `global *` declares no name of its own, so renamed globals stay
        // legal.
        assert_lua55_renames(&[
            (
                "\
global *
g = 1
print(g)
",
                "global*l=1 print(l)",
            ),
            (
                "\
local g = f()
global *
g = 1
",
                "local l=f()global*l=1",
            ),
        ]);
    }

    #[test]
    fn renames_inside_a_function_embedded_in_a_type() {
        assert_luau_renames(&[
            (
                "type T = typeof(function(x) return x end)\n",
                "type T=typeof(function(l)return l end)",
            ),
            (
                "type T<U = typeof(function(x) return x end)> = U\n",
                "type T<U=typeof(function(l)return l end)> =U",
            ),
            (
                "type function T() return typeof(function(x) return x end) end\n",
                "type function T()return typeof(function(l)return l end)end",
            ),
            (
                "type T = { f: typeof(function(x) return x end)? }\n",
                "type T={f:typeof(function(l)return l end)?}",
            ),
            (
                "type T = (typeof(function(x) return x end)) -> ()\n",
                "type T=(typeof(function(l)return l end))->()",
            ),
            (
                "type T = A<typeof(function(x) return x end)>\n",
                "type T=A<typeof(function(l)return l end)>",
            ),
            (
                "return 1 :: typeof(function(x) return x end)\n",
                "return 1::typeof(function(l)return l end)",
            ),
            (
                "return f<<typeof(function(x) return x end)>>\n",
                "return f<<typeof(function(l)return l end)>>",
            ),
            (
                "return f<<typeof(function(x) return x end)>>()\n",
                "return f<<typeof(function(l)return l end)>>()",
            ),
            ("f<<typeof(function(x) return x end)>>()\n", "f<<typeof(function(l)return l end)>>()"),
            (
                "return t:m<<typeof(function(x) return x end)>>()\n",
                "return t:m<<typeof(function(l)return l end)>>()",
            ),
        ]);
    }

    #[test]
    fn bindings_in_a_type_keep_their_place_in_walk_order() {
        // Binding ids follow visit order, so visiting a type out of turn hands
        // the renamer another binding's name.
        assert_luau_renames(&[
            (
                "f<<typeof(function(a, b) return a, b end)>>(function(c) return c end)\n",
                "f<<typeof(function(l,u)return l,u end)>>(function(l)return l end)",
            ),
            (
                "t:m<<typeof(function(a, b) return a, b end)>>(function(c) return c end)\n",
                "t:m<<typeof(function(l,u)return l,u end)>>(function(l)return l end)",
            ),
            (
                "return (function(a, b) return a, b end) :: typeof(function(c) return c end)\n",
                "return(function(l,u)return l,u end)::typeof(function(l)return l end)",
            ),
            (
                "type T<U = typeof(function(a, b) return a, b end)> = typeof(function(c) return c end)\n",
                "type T<U=typeof(function(l,u)return l,u end)> =typeof(function(l)return l end)",
            ),
            (
                "\
type T = typeof(function(longparam) return longparam end)
local after = f()
return after
",
                "type T=typeof(function(l)return l end)local l=f()return l",
            ),
        ]);
    }

    #[test]
    fn bindings_in_an_assignment_keep_their_place_in_walk_order() {
        // Binding ids follow visit order, and the renamer visits an
        // assignment's targets before its values. Visiting them out of turn
        // hands a parameter another binding's name, which can capture `x`.
        assert_luau_renames(&[
            (
                "\
local x = f()
t[function(a) return x, a end] = function(b) return b end
return x
",
                "local l=f()t[function(u)return l,u end]=function(l)return l end return l",
            ),
            (
                "\
local x = f()
g(function(a) return a end).k = function(b) return b + x end
return x
",
                "local l=f()g(function(l)return l end).k=function(u)return u+l end return l",
            ),
        ]);
    }

    #[test]
    fn typeof_read_keeps_a_local_live_in_its_scope() {
        assert_luau_renames(&[(
            "\
local outer = f()
do
local inner = f()
type T = typeof(outer)
print(inner)
end
print(outer)
",
            "local l=f()do local u=f()type T=typeof(l)print(u)end print(l)",
        )]);
    }

    #[test]
    fn typeof_read_of_a_global_is_not_captured() {
        assert_luau_renames(&[(
            "\
local function f()
local longname = g()
type T = typeof(l)
return longname
end
return f
",
            "local function u()local u=g()type T=typeof(l)return u end return u",
        )]);
    }

    #[test]
    fn named_vararg_binds_its_name_in_the_body() {
        assert_lua55_renames(&[
            (
                "\
local args = f()
local function g(...args) return args end
return args, g
",
                "local l=f()local function u(...l)return l end return l,u",
            ),
            (
                "\
local function g(a, ...rest) return rest, a end
return g
",
                "local function l(l,...u)return u,l end return l",
            ),
        ]);
    }

    #[test]
    fn unreferenced_param_never_captures_live_upvalue() {
        // An unreferenced param must not get the name of an upvalue read in
        // the body, which would capture it.
        let result = apply(
            "\
local cache = string.rep
function W.new(p)
\tprint(cache)
end
cache = f()
",
        );
        // The upvalue is the outer local, so the param must not wear its name.
        let (_, after_declaration) = result.split_once("function W.new(").expect("decl kept");
        let param = after_declaration.chars().next();
        let outer = result.chars().nth(6);
        assert_ne!(param, outer, "param must not shadow the captured upvalue: {result}");
    }

    #[test]
    fn nested_method_self_is_not_the_enclosing_self_parameter() {
        let result = apply(
            "\
function M.f(self)
  function t:g() return self end
  return self
end
",
        );
        assert!(
            result.contains(":g()return self"),
            "`self` in a `:` method is its own receiver, not the renamed parameter: {result}"
        );
    }

    #[test]
    fn lua51_vararg_function_keeps_its_implicit_arg() {
        let result = apply_version(
            "\
local arg = g()
local function f(...) return arg end
return f, arg
",
            LuaVersion::Lua51,
        );
        assert!(
            result.contains("return arg"),
            "`arg` in a 5.1 vararg function is its implicit table, not the outer local: {result}"
        );
    }

    #[test]
    fn renamed_local_never_wears_an_exported_name() {
        let result = apply_luau(
            "\
export local l = 1
local longname = f()
print(l, longname)
",
        );
        assert!(
            !result.contains("local l=f()"),
            "a renamed local must not shadow the exported `l`: {result}"
        );
    }

    #[test]
    fn lua51_implicit_arg_wins_over_a_parameter_named_arg() {
        let result = apply_version(
            "\
local function f(arg, ...) return arg end
return f
",
            LuaVersion::Lua51,
        );
        assert!(result.contains("return arg"), "{result}");
    }

    #[test]
    fn local_named_self_is_an_ordinary_local() {
        assert_luau_renames(&[(
            "\
local self = f()
print(self, self)
",
            "local l=f()print(l,l)",
        )]);
    }

    #[test]
    fn typeof_in_every_annotation_follows_the_rename() {
        for source in [
            "\
local outer = f()
local function g(a: typeof(outer)) return a end
return g, outer
",
            "\
local outer = f()
local function g(...: typeof(outer)) return ... end
return g, outer
",
            "\
local outer = f()
local function g(): typeof(outer) return outer end
return g
",
            "\
local outer = f()
for i: typeof(outer) = 1, 2 do print(i) end
return outer
",
            "\
local outer = f()
for k: typeof(outer) in g() do print(k) end
return outer
",
        ] {
            let result = apply_luau(source);
            assert!(!result.contains("typeof(outer)"), "{source}: {result}");
        }
    }

    #[test]
    fn typeof_in_a_signature_reads_the_scope_around_the_function() {
        for source in [
            "\
local a = f()
local function g(a: number, b: typeof(a)) return b end
return g, a
",
            "\
local a = f()
local function g(a: number): typeof(a) return a end
return g, a
",
            "\
local g = f()
local function g(z: typeof(g)) return z end
return g
",
            "\
local k = f()
for k, v: typeof(k) in g() do print(k, v) end
return k
",
        ] {
            let result = apply_luau(source);
            assert!(result.contains("typeof(l)"), "{source}: {result}");
        }
    }

    #[test]
    fn typeof_in_a_local_annotation_follows_the_rename() {
        let result = apply_luau(
            "\
local outer = f()
local copy: typeof(outer) = outer
return copy, outer
",
        );
        assert!(
            !result.contains("typeof(outer)"),
            "`typeof` must name the local's new name, or the output fails type checking: {result}"
        );
    }
}
