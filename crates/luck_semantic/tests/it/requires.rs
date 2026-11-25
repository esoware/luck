use luck_allocator::Allocator;
use luck_ast::GetSpan as _;
use luck_semantic::SemanticBuilder;

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
_G:require(\"i\")
_G.require.j(\"k\")
return require(require(\"f\").g)";
    let semantic = semantic_for(&allocator, source);
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
            ("require(require(\"f\").g)", None),
            ("require(\"f\")", Some(&b"f"[..])),
        ]
    );
    for call in ["require(\"a\")", "_G.require(\"a\")"] {
        let shadowed = format!("require = nil\n{call}");
        let found = semantic_for(&allocator, &shadowed);
        assert!(found.requires().is_empty(), "{shadowed}");
    }
    let unasked = SemanticBuilder::new().build(parse(&allocator, "require(\"a\")"));
    assert!(unasked.semantic.requires().is_empty());
}

#[test]
fn a_require_site_knows_whether_it_is_in_a_type() {
    let allocator = Allocator::new();
    let source = "\
type T = typeof(require(\"a\"))
local b: typeof(require(\"b\")) = require(\"c\")
return require(\"d\")";
    let semantic = semantic_for(&allocator, source);
    let sites: Vec<(&[u8], bool)> = semantic
        .requires()
        .iter()
        .map(|site| (site.path.expect("a literal path").value.as_bytes(), site.is_in_type))
        .collect();
    assert_eq!(sites, [(&b"a"[..], true), (b"b", true), (b"c", false), (b"d", false)]);
}
