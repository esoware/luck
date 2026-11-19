//! A tree built for some names records what the whole tree records of them,
//! so one built for every name the chunk uses is the whole tree.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use luck_allocator::Allocator;
use luck_ast::Name;
use luck_semantic::{ScopeTree, SymbolId};
use luck_testgen::{collect_fixture_files, generate_full, generate_runtime_safe, is_error_fixture};
use luck_token::LuaVersion;

/// Each fact `tree` holds about the names `is_kept` picks.
///
/// A symbol goes by where it's defined, since a tree of fewer names numbers
/// them otherwise. Unless `is_every_name`, an `_ENV`'s upvalue mark is left
/// out, since only the names the tree records capture it there.
fn facts(tree: &ScopeTree<'_>, is_kept: impl Fn(&str) -> bool, is_every_name: bool) -> Vec<String> {
    let definition = |id: SymbolId| {
        let symbol = tree.symbol(id);
        (tree.name(symbol.name), symbol.definition_span)
    };
    let mut facts = vec![format!("{:?} env assigned {}", tree.scopes(), tree.is_env_assigned())];
    for (id, name) in tree.names().filter(|&(_, name)| is_kept(name)) {
        facts.push(format!(
            "name {name} assigned {} declared {}",
            tree.is_assigned_global(id),
            tree.is_declared_global(id)
        ));
    }
    for symbol in tree.symbols().iter().filter(|symbol| is_kept(tree.name(symbol.name))) {
        let is_upvalue =
            (is_every_name || !tree.is_env_binding(symbol.id)).then_some(symbol.is_upvalue);
        let references: Vec<_> =
            tree.resolved_references(symbol.id).iter().map(|&id| tree.reference(id).span).collect();
        facts.push(format!(
            "symbol {:?} in {:?} {:?} shadows {:?} upvalue {is_upvalue:?} modified {} exported {} \
             close {} references {references:?}",
            definition(symbol.id),
            symbol.scope,
            symbol.kind,
            symbol.shadows.map(definition),
            symbol.is_modified,
            symbol.is_exported,
            symbol.is_close,
        ));
    }
    for reference in tree.references() {
        let name = tree.name(reference.name);
        if !is_kept(name) {
            continue;
        }
        let found = tree.reference_of(&Name::new(name.into(), None, reference.span));
        facts.push(format!(
            "reference {name} {:?} in {:?} {:?} to {:?} env {:?} declared {} global {} host {} \
             found {:?} local {}",
            reference.span,
            reference.scope,
            reference.kind,
            reference.resolved.map(definition),
            reference.env.map(definition),
            reference.is_declared_global,
            tree.is_global(reference),
            tree.is_host_global(reference),
            found.map(|id| tree.reference(id).span),
            tree.resolves_to_local(&Name::new(name.into(), None, reference.span)),
        ));
    }
    facts
}

/// Checks that every tree of some names built from `source` as `version`
/// agrees with the whole tree, and says whether it parses that way.
fn check(source: &str, version: LuaVersion) -> bool {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, source, version);
    if !parsed.errors.is_empty() {
        return false;
    }
    let counts = parsed.counts;
    let chunk = allocator.alloc(parsed.chunk);
    let whole = ScopeTree::build(chunk, counts);
    let every_name: Vec<&str> = whole.names().map(|(_, name)| name).collect();

    let rebuilt = ScopeTree::build_for_names(chunk, &every_name, &mut ());
    assert_eq!(
        format!("{:?}{:?}", rebuilt.symbols(), rebuilt.references()),
        format!("{:?}{:?}", whole.symbols(), whole.references()),
        "{version:?}\n{source}"
    );
    assert_eq!(
        facts(&rebuilt, |_| true, true),
        facts(&whole, |_| true, true),
        "{version:?}\n{source}"
    );

    let [evens, odds] =
        [0, 1].map(|first| every_name.iter().copied().skip(first).step_by(2).collect::<Vec<_>>());
    for names in [&["require", "_G", "package", "module"][..], &evens, &odds, &[]] {
        let some = ScopeTree::build_for_names(chunk, names, &mut ());
        let is_kept = |name: &str| name == "_ENV" || names.contains(&name);
        assert_eq!(
            facts(&some, is_kept, false),
            facts(&whole, is_kept, false),
            "{version:?} {names:?}\n{source}"
        );
        assert!(some.names().all(|(_, name)| is_kept(name)), "{version:?} {names:?}\n{source}");
    }
    true
}

#[test]
fn trees_of_some_names_agree_with_the_whole_tree_on_scoping_corners() {
    let sources = [
        "\
local r = require
require(\"a\")
_G.require(\"b\")
package.loaded.x = 1
do
    local require = print
    require(\"c\")
end
local _ENV = { require = print }
require(\"d\")
module(\"m\")
return function(...) return ..., arg, r end
",
        "\
_ENV = {}
x = require
",
        "\
local t = {}
function t:m(a) return self, a end
function t.n(self) return self end
local x <close> = nil
local x <const> = x
repeat local done = x until done
for i = 1, 2 do local i = i end
for k, v in pairs(t) do print(k, v) end
local function f(...) return f, ... end
",
        "\
local x = 1
global function f(...v) return x, v end
do
    global x
    x = 2
end
global *
y = x
global g = g
",
        "\
type require = number
type function f(t)
    return require(types), g(t)
end
type function g(t) return t end
local function h(a: number, b: typeof(a)): typeof(h) return b end
export local a = require(\"./a\")
export function e() a = 1 end
local a = 2
package.loaded.x ..= \"y\"
",
    ];
    for source in sources {
        let parsed = LuaVersion::ALL.into_iter().filter(|&version| check(source, version));
        assert!(parsed.count() > 0, "{source}");
    }
}

#[test]
fn trees_of_some_names_agree_with_the_whole_tree_on_every_fixture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let mut checked = 0usize;
    for path in collect_fixture_files(&root).unwrap() {
        let relative = path.strip_prefix(&root).unwrap();
        let is_source = matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau"));
        if !is_source || is_error_fixture(relative) {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        let parsed = LuaVersion::ALL.into_iter().filter(|&version| check(&source, version));
        checked = checked.saturating_add(parsed.count());
    }
    assert!(checked > 0, "no fixtures found under {}", root.display());
}

#[test]
fn trees_of_some_names_agree_with_the_whole_tree_on_generated_programs() {
    for version in LuaVersion::ALL {
        for seed in 0..40 {
            assert!(check(&generate_full(seed, version, 25), version));
            assert!(check(&generate_runtime_safe(seed, version, 25), version));
        }
    }
}
