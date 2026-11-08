mod analyzer;
mod candidate_pool;
mod globals;
mod renamer;
mod scope_set;

use luck_ast::{Block, Transform as _, Visitor as _};
use luck_token::LuaVersion;

use self::analyzer::Analyzer;
use self::globals::renamable_globals;
use self::renamer::Renamer;

/// Renames every local to the shortest name its scopes allow, and with
/// `rename_globals` the globals the chunk defines before reading them too.
pub(crate) fn rename(block: Block, version: LuaVersion, rename_globals: bool) -> Block {
    // Opt-in, since renamed globals end up under new `_G` keys.
    let root_globals = if rename_globals { renamable_globals(&block, version) } else { Vec::new() };

    // The analyzer resolves names with the same scoping rules as the renamer,
    // so a reference that resolves to no binding is a global. A flat name set
    // can't tell a global read from a same-named local elsewhere in the file.
    let mut analyzer = Analyzer::new(version);
    analyzer.declare_root_globals(&root_globals);
    analyzer.visit_block(&block);
    let binding_names = analyzer.binding_names();

    let mut renamer = Renamer::new(binding_names);
    renamer.declare_root_globals(&root_globals);
    renamer.transform_block(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    /// Renames `source`, globals included, and checks the result reparses.
    fn apply_version(source: &str, version: LuaVersion) -> String {
        let output = run_pass(source, version, |block| rename(block, version, true));
        assert_reparses(&output, version);
        output
    }

    #[test]
    fn renames_locals_to_short_names() {
        let result = apply("local longname = 1\nreturn longname\n");
        assert!(!result.contains("longname"), "Local should be renamed: {result}");
    }

    #[test]
    fn preserves_globals() {
        let result = apply("print(42)\n");
        assert!(result.contains("print"), "Global should not be renamed: {result}");
    }

    #[test]
    fn renames_function_params() {
        let result = apply("local function foo(longparam)\n  return longparam\nend\nreturn foo\n");
        assert!(!result.contains("longparam"), "Param should be renamed: {result}");
    }

    #[test]
    fn preserves_field_names() {
        let result = apply("local t = {}\nt.field = 1\nreturn t.field\n");
        assert!(result.contains("field"), "Field name should not be renamed: {result}");
    }

    #[test]
    fn renames_a_nested_scope_to_valid_lua() {
        apply("local x = 1\ndo\n  local y = 2\n  print(y)\nend\nreturn x\n");
    }

    #[test]
    fn most_frequent_local_gets_shortest_name() {
        let result = apply(
            "local rare = 1\nlocal frequent = 2\nprint(frequent)\nprint(frequent)\nprint(frequent)\nprint(rare)\nreturn frequent, rare\n",
        );
        assert!(!result.contains("frequent"), "'frequent' should be renamed: {result}");
        assert!(!result.contains("rare"), "'rare' should be renamed: {result}");
    }

    #[test]
    fn preserves_self_parameter() {
        let result = apply("local t = {}\nfunction t:method()\n  return self\nend\nreturn t\n");
        assert!(result.contains("self"), "'self' should not be renamed: {result}");
    }

    #[test]
    fn sibling_scopes_reuse_names() {
        let result = apply(
            "do\n  local longvar = 1\n  print(longvar)\nend\ndo\n  local another = 2\n  print(another)\nend\n",
        );
        assert!(
            !result.contains("longvar") && !result.contains("another"),
            "Both should be renamed: {result}"
        );
    }

    #[test]
    fn renames_nested_function_locals_to_valid_lua() {
        apply(
            "local outer = 1\nlocal function f()\n  local inner = 2\n  print(inner, inner, inner, inner, inner)\nend\nreturn outer, f\n",
        );
    }

    #[test]
    fn locals_can_shadow_unused_globals() {
        // 'l' and 'u' are globals at top level but not inside f.
        let result = apply(concat!(
            "l(1)\nu(2)\n",
            "local function f()\n",
            "  local longvar1 = 3\n  local longvar2 = 4\n",
            "  return longvar1 + longvar2\nend\n",
            "return f\n",
        ));
        assert!(!result.contains("longvar"), "locals should be renamed: {result}");
    }

    #[test]
    fn locals_avoid_globals_used_in_same_scope() {
        // 'l' is a global called inside f, so locals must not shadow it.
        let result = apply(concat!(
            "local function f()\n",
            "  local longvar = 1\n  l(longvar)\n  return longvar\n",
            "end\nreturn f\n",
        ));
        assert!(result.contains("l("), "global 'l' must be preserved: {result}");
        assert!(!result.contains("longvar"), "longvar should be renamed: {result}");
    }

    #[test]
    fn renames_inside_global_function_body() {
        // The analyzer and the renamer must both walk `global function` bodies,
        // or the binding ids fall out of step.
        let result = apply_version(
            "global function f(longparam)\n  return longparam\nend\n",
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
        let result = apply_luau("h = (g :: any)\ng = 1\nprint(h, g)\n");
        assert!(result.contains('g'), "global 'g' read via typecast must be preserved: {result}");
    }

    #[test]
    fn global_read_via_compound_index_is_not_assign_first() {
        // `g` is read as the index of a compound target before it's assigned,
        // so it's a real global and must keep its name, at any nesting depth.
        for (source, expected) in [
            ("t[g] += 1\ng = 1\nprint(t, g)\n", "t[g]+=1 g=1 print(t,g)"),
            ("do t[g] += 1 end\ng = 1\nprint(t, g)\n", "do t[g]+=1 end g=1 print(t,g)"),
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
            ("t[g] = 1\ng = 1\nprint(t, g)\n", "t[g]=1 g=1 print(t,g)"),
            ("g[1] = 2\ng = 1\nprint(g)\n", "g[1]=2 g=1 print(g)"),
            ("g.a.b = 1\ng = 1\nprint(g)\n", "g.a.b=1 g=1 print(g)"),
            ("g[1].b = 1\ng = 1\nprint(g)\n", "g[1].b=1 g=1 print(g)"),
            ("t[g].b = 1\ng = 1\nprint(t, g)\n", "t[g].b=1 g=1 print(t,g)"),
            ("f(g).x = 1\ng = 1\nprint(g)\n", "f(g).x=1 g=1 print(g)"),
            ("(g).x = 1\ng = 1\nprint(g)\n", "(g).x=1 g=1 print(g)"),
            ("t[g], g = 1, 2\nprint(t, g)\n", "t[g],g=1,2 print(t,g)"),
        ]);
    }

    #[test]
    fn global_read_by_later_target_is_not_assign_first() {
        // Every target's prefix and index is evaluated before any store, so `g`
        // is read here even though its own target comes first.
        assert_luau_renames(&[
            ("g, t[g] = 1, 2\nprint(t, g)\n", "g,t[g]=1,2 print(t,g)"),
            ("g, g[1] = 1, 2\nprint(g)\n", "g,g[1]=1,2 print(g)"),
            ("g, g.x = 1, 2\nprint(g)\n", "g,g.x=1,2 print(g)"),
        ]);
    }

    #[test]
    fn global_read_via_nested_return_is_not_assign_first() {
        // A return sits outside its block's statement list, and an early one
        // reads `g` before the assignment that follows the block.
        assert_luau_renames(&[
            ("do return g end\ng = 1\nprint(g)\n", "do return g end g=1 print(g)"),
            ("if c then return g end\ng = 1\nprint(g)\n", "if c then return g end g=1 print(g)"),
            (
                "if c then print(1) elseif d then return g end\ng = 1\nprint(g)\n",
                "if c then print(1)elseif d then return g end g=1 print(g)",
            ),
            (
                "if c then print(1) else return g end\ng = 1\nprint(g)\n",
                "if c then print(1)else return g end g=1 print(g)",
            ),
            ("while c do return g end\ng = 1\nprint(g)\n", "while c do return g end g=1 print(g)"),
            ("repeat return g until c\ng = 1\nprint(g)\n", "repeat return g until c g=1 print(g)"),
            (
                "for i = 1, 2 do return g end\ng = 1\nprint(g)\n",
                "for l=1,2 do return g end g=1 print(g)",
            ),
            (
                "for k in x do return g end\ng = 1\nprint(g)\n",
                "for l in x do return g end g=1 print(g)",
            ),
        ]);
    }

    #[test]
    fn global_read_via_function_name_is_not_assign_first() {
        // `function g.f()` and `function g:f()` read `g` to store into it.
        assert_luau_renames(&[
            ("function g.x() end\ng = 1\nprint(g)\n", "function g.x()end g=1 print(g)"),
            ("function g:m() end\ng = 1\nprint(g)\n", "function g:m()end g=1 print(g)"),
            ("function g.a.b() end\ng = 1\nprint(g)\n", "function g.a.b()end g=1 print(g)"),
            (
                "do function g.x() end end\ng = 1\nprint(g)\n",
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
            ("g = 1\nt[g] = 2\nprint(t, g)\n", "l=1 t[l]=2 print(t,l)"),
            ("g = 1\ng.a.b = 2\nprint(g)\n", "l=1 l.a.b=2 print(l)"),
            ("g = 1\nfunction g.x() end\nprint(g)\n", "l=1 function l.x()end print(l)"),
            ("do function g() end end\ng = 1\nprint(g)\n", "do function l()end end l=1 print(l)"),
            (
                "h = function() return g end\ng = 1\nprint(h, g)\n",
                "u=function()return l end l=1 print(u,l)",
            ),
            ("h, t[g] = 1, 2\nprint(h, t)\n", "l,t[g]=1,2 print(l,t)"),
        ]);
    }

    #[test]
    fn global_read_before_function_definition_is_not_renamed() {
        // Same reads as the assign-first check, at the same depths.
        assert_luau_renames(&[
            ("print(g)\nfunction g() end\n", "print(g)function g()end"),
            ("x = g\nfunction g() end\n", "l=g function g()end"),
            ("t[g] = 1\nfunction g() end\n", "t[g]=1 function g()end"),
            ("g.x = 1\nfunction g() end\n", "g.x=1 function g()end"),
            ("g[1] = 1\nfunction g() end\n", "g[1]=1 function g()end"),
            ("g, t[g] = 1, 2\nfunction g() end\n", "g,t[g]=1,2 function g()end"),
            ("g += 1\nfunction g() end\n", "g+=1 function g()end"),
            ("function g.x() end\nfunction g() end\n", "function g.x()end function g()end"),
            ("function g:m() end\nfunction g() end\n", "function g:m()end function g()end"),
            (
                "if c then return g end\nfunction g() end\n",
                "if c then return g end function g()end",
            ),
            ("do print(g) end\nfunction g() end\n", "do print(g)end function g()end"),
            (
                "print(g)\nfunction g() end\nfunction g() end\n",
                "print(g)function g()end function g()end",
            ),
            (
                "print(g)\nfunction g() end\ng = 1\nprint(g)\n",
                "print(g)function g()end g=1 print(g)",
            ),
            (
                "print(g)\ndo local g = 1 end\nfunction g() end\n",
                "print(g)do local l=1 end function g()end",
            ),
        ]);
    }

    #[test]
    fn nested_read_before_the_local_it_shares_a_name_with_is_a_global_read() {
        assert_luau_renames(&[
            (
                "do print(g)\nlocal g = 1 end\nfunction g() end\n",
                "do print(g)local l=1 end function g()end",
            ),
            ("do local g = g end\nfunction g() end\n", "do local l=g end function g()end"),
            (
                "do do local g = f() end\nprint(g) end\nfunction g() end\n",
                "do do local l=f()end print(g)end function g()end",
            ),
            ("for g = g, 2 do end\nfunction g() end\n", "for l=g,2 do end function g()end"),
            ("for g in g do end\nfunction g() end\n", "for l in g do end function g()end"),
            (
                "repeat print(g)\nlocal g = f() until g\nfunction g() end\n",
                "repeat print(g)local l=f()until l function g()end",
            ),
        ]);
    }

    #[test]
    fn read_of_a_nested_local_is_not_a_read_of_the_global() {
        assert_luau_renames(&[
            (
                "do local g = f()\nprint(g) end\nfunction g() end\n",
                "do local l=f()print(l)end function l()end",
            ),
            (
                "do local function g() end\ng() end\nfunction g() end\n",
                "do local function l()end l()end function l()end",
            ),
            (
                "for g = 1, 2 do print(g) end\nfunction g() end\n",
                "for l=1,2 do print(l)end function l()end",
            ),
            (
                "for g in x do print(g) end\nfunction g() end\n",
                "for l in x do print(l)end function l()end",
            ),
            (
                "repeat local g = f() until g\nfunction g() end\n",
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
                "g = 1\ndo local g = f() end\nprint(g)\nfunction g() end\n",
                "l=1 do local l=f()end print(l)function l()end",
            ),
            (
                "a = 1\nprint(a)\nprint(a)\ng = 1\nprint(g)\nfunction g() end\n",
                "u=1 print(u)print(u)l=1 print(l)function l()end",
            ),
        ]);
    }

    #[test]
    fn function_defined_before_any_read_stays_renamed() {
        // A read after the definition and a function body's deferred read both
        // leave `g` renamed, and so does an assignment that got there first.
        assert_luau_renames(&[
            ("function g() end\nprint(g)\n", "function l()end print(l)"),
            ("function g() return g end\n", "function l()return l end"),
            (
                "function g() end\nprint(g)\nfunction g() end\n",
                "function l()end print(l)function l()end",
            ),
            (
                "h = function() return g end\nfunction g() end\n",
                "u=function()return l end function l()end",
            ),
            (
                "function f() return g end\nf()\nfunction g() end\n",
                "function l()return u end l()function u()end",
            ),
            ("g = 1\nfunction g() end\nprint(g)\n", "l=1 function l()end print(l)"),
            ("g = 1\nprint(g)\nfunction g() end\n", "l=1 print(l)function l()end"),
            ("do local g = 1 end\nfunction g() end\n", "do local l=1 end function l()end"),
        ]);
    }

    #[test]
    fn deferred_global_read_is_left_as_it_was() {
        let result = apply("function f() return g end\nf()\ng = 1\n");
        assert_eq!(result, "function l()return u end l()u=1");
    }

    const ENV_VERSIONS: [LuaVersion; 4] =
        [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55];

    #[test]
    fn env_is_never_renamed_on_versions_with_an_env_upvalue() {
        for version in ENV_VERSIONS {
            for (source, expected) in [
                ("_ENV = {}\nprint(1)\n", "_ENV={}print(1)"),
                ("_ENV = {}\n_ENV = {}\n", "_ENV={}_ENV={}"),
                ("_ENV, x = {}, 1\nprint(x)\n", "_ENV,l={},1 print(l)"),
                ("function _ENV() end\n", "function _ENV()end"),
                ("function _ENV.f() end\n", "function _ENV.f()end"),
                ("local _ENV = {}\nprint(1)\n", "local _ENV={}print(1)"),
                ("local _ENV = {}\n_ENV = {}\n", "local _ENV={}_ENV={}"),
                (
                    "do local _ENV = {}\n_ENV = {} end\n_ENV = {}\n",
                    "do local _ENV={}_ENV={}end _ENV={}",
                ),
                ("local function _ENV() end\n", "local function _ENV()end"),
                (
                    "local function f(_ENV) return x end\nreturn f\n",
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
                ("_ENV = 1\nprint(_ENV)\n", "l=1 print(l)"),
                ("function _ENV() end\nprint(_ENV)\n", "function l()end print(l)"),
                ("local _ENV = 1\nprint(_ENV)\n", "local l=1 print(l)"),
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
            ("global g\ng = 1\n", "global g g=1"),
            ("g = 1\nglobal g\n", "g=1 global g"),
            ("g = 1\nglobal g\ng = 2\n", "g=1 global g g=2"),
            ("global g = 1\ng = 2\n", "global g=1 g=2"),
            ("global a, g\ng = 1\na = 2\n", "global a,g g=1 a=2"),
            ("global <const> g = 1\n", "global g<const> =1"),
            ("function g() end\nglobal g\n", "function g()end global g"),
            ("global g\nfunction g() end\n", "global g function g()end"),
        ]);
    }

    #[test]
    fn name_of_global_function_is_not_renamed() {
        assert_lua55_renames(&[
            ("g = 1\nglobal function g() end\n", "g=1 global function g()end"),
            (
                "function g() end\nglobal function g() end\n",
                "function g()end global function g()end",
            ),
            ("global function g() return g end\ng()\n", "global function g()return g end g()"),
        ]);
    }

    #[test]
    fn global_declaration_pins_its_name_from_any_depth() {
        assert_lua55_renames(&[
            ("g = 1\ndo global g\ng = 2 end\n", "g=1 do global g g=2 end"),
            ("g = 1\nif c then global g end\n", "g=1 if c then global g end"),
            ("g = 1\nfunction f() global g\ng = 2 end\n", "g=1 function l()global g g=2 end"),
            (
                "function g() end\nlocal function f() global function g() end end\nreturn f\n",
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
                "local g = f()\nprint(g)\nglobal g, print\ng = 2\nprint(g)\n",
                "local l=f()print(l)global g,print g=2 print(g)",
            ),
            (
                "local g = f()\ndo global g\ng = 2 end\nprint(g)\n",
                "local l=f()do global g g=2 end print(l)",
            ),
            (
                "local g = f()\nglobal function g() return g end\n",
                "local l=f()global function g()return g end",
            ),
        ]);
    }

    #[test]
    fn global_declaration_takes_effect_after_its_initializers() {
        assert_lua55_renames(&[(
            "local g = f()\nglobal g = g\ng = 2\n",
            "local l=f()global g=l g=2",
        )]);
    }

    #[test]
    fn local_declared_after_a_global_declaration_is_still_renamed() {
        assert_lua55_renames(&[(
            "global g, print\ndo\nlocal g = f()\nprint(g)\nend\ng = 1\n",
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
                "local longname = f()\nglobal l, print\nprint(longname)\n",
                "local u=f()global l,print print(u)",
            ),
            (
                "local longname = f()\ndo global l, print\nprint(longname) end\n",
                "local u=f()do global l,print print(u)end",
            ),
            (
                "global l, print, f\ndo\nlocal longname = f()\nprint(longname, l)\nend\n",
                "global l,print,f do local u=f()print(u,l)end",
            ),
            (
                "local longname = f()\nglobal function l() end\nreturn longname\n",
                "local u=f()global function l()end return u",
            ),
        ]);
    }

    #[test]
    fn collective_global_declaration_leaves_every_name_free() {
        // `global *` declares no name of its own, so renamed globals stay
        // legal.
        assert_lua55_renames(&[
            ("global *\ng = 1\nprint(g)\n", "global*l=1 print(l)"),
            ("local g = f()\nglobal *\ng = 1\n", "local l=f()global*l=1"),
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
                "type T = typeof(function(longparam) return longparam end)\nlocal after = f()\nreturn after\n",
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
                "local x = f()\nt[function(a) return x, a end] = function(b) return b end\nreturn x\n",
                "local l=f()t[function(u)return l,u end]=function(l)return l end return l",
            ),
            (
                "local x = f()\ng(function(a) return a end).k = function(b) return b + x end\nreturn x\n",
                "local l=f()g(function(l)return l end).k=function(u)return u+l end return l",
            ),
        ]);
    }

    #[test]
    fn typeof_read_keeps_a_local_live_in_its_scope() {
        assert_luau_renames(&[(
            "local outer = f()\ndo\nlocal inner = f()\ntype T = typeof(outer)\nprint(inner)\nend\nprint(outer)\n",
            "local l=f()do local u=f()type T=typeof(l)print(u)end print(l)",
        )]);
    }

    #[test]
    fn typeof_read_of_a_global_is_not_captured() {
        assert_luau_renames(&[(
            "local function f()\nlocal longname = g()\ntype T = typeof(l)\nreturn longname\nend\nreturn f\n",
            "local function u()local u=g()type T=typeof(l)return u end return u",
        )]);
    }

    #[test]
    fn unreferenced_param_never_captures_live_upvalue() {
        // An unreferenced param must not get the name of an upvalue read in
        // the body, which would capture it.
        let result = apply(
            "local cache = string.rep
function W.new(p)
	print(cache)
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
}
