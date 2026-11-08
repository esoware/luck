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
