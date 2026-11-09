use luck_parser::parse;
use luck_token::LuaVersion;

#[test]
fn owned_source_moves_into_the_result_without_a_copy() {
    for text in ["return 1", "local = 5"] {
        let mut source = String::with_capacity(256);
        source.push_str(text);
        let pointer = source.as_ptr();
        let capacity = source.capacity();
        for _ in 0u8..3 {
            let parsed = parse(source, LuaVersion::Lua54);
            assert_eq!(parsed.source.as_ptr(), pointer);
            assert_eq!(parsed.source.capacity(), capacity);
            assert_eq!(parsed.source, text);
            assert_eq!(parsed.errors.is_empty(), text == "return 1", "{:?}", parsed.errors);
            source = parsed.source;
        }
    }
}

#[test]
fn declaration_without_values_spans_its_annotation_and_attribute() {
    for (source, version) in [
        ("local x <const>", LuaVersion::Lua54),
        ("local a, b <close>", LuaVersion::Lua54),
        ("global x <const>", LuaVersion::Lua55),
        ("local x: number", LuaVersion::Luau),
    ] {
        let result = parse(source, version);
        assert!(result.errors.is_empty(), "{source}: {:?}", result.errors);
        let statement_span = result.block.stmts[0].span();
        assert_eq!(statement_span.text(source), source, "the statement must span all of {source}");
    }
}
