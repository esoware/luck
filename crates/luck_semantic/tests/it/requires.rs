use luck_allocator::Allocator;
use luck_ast::GetSpan as _;
use luck_semantic::SemanticBuilder;
use luck_token::LuaVersion;

use crate::common::{parse, semantic_for};

#[test]
fn require_sites_are_the_calls_of_the_host_require() {
    let allocator = Allocator::new();
    let source = "\
local a = require(\"a\")
require \"b\"
require(x)
require(\"c\", 2)
obj:require(\"d\")
do
    local require = print
    require(\"e\")
end
_G.require(\"g\")
_ENV[\"require\"] \"h\"
_G:require(\"i\")
_G.require.j(\"k\")
return require(require(\"f\").g)";
    let semantic = semantic_for(&allocator, source, LuaVersion::Lua54);
    let sites: Vec<(&str, Option<&[u8]>)> = semantic
        .requires()
        .iter()
        .map(|site| {
            let call = site.call.span().text(source.as_bytes());
            (str::from_utf8(call).unwrap(), site.path.map(|path| path.value.as_bytes()))
        })
        .collect();
    assert_eq!(
        sites,
        [
            ("require(\"a\")", Some(&b"a"[..])),
            ("require \"b\"", Some(&b"b"[..])),
            ("require(x)", None),
            ("require(\"c\", 2)", None),
            ("_G.require(\"g\")", Some(&b"g"[..])),
            ("_ENV[\"require\"] \"h\"", Some(&b"h"[..])),
            ("require(require(\"f\").g)", None),
            ("require(\"f\")", Some(&b"f"[..])),
        ]
    );
    for shadowing in ["require = nil", "local _ENV = {}", "_ENV = {}"] {
        for call in ["require(\"a\")", "_G.require(\"a\")"] {
            let shadowed = format!("{shadowing}\n{call}");
            let found = semantic_for(&allocator, &shadowed, LuaVersion::Lua54);
            assert!(found.requires().is_empty(), "{shadowed}");
        }
    }
    let in_lua51 = semantic_for(&allocator, "_ENV.require(\"a\")", LuaVersion::Lua51);
    assert!(in_lua51.requires().is_empty(), "Lua 5.1's `_ENV` is a plain global");
    let unasked =
        SemanticBuilder::new().build(parse(&allocator, "require(\"a\")", LuaVersion::Lua54));
    assert!(unasked.semantic.requires().is_empty());
}
