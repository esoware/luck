use luck_parser::ParseResult;
use luck_token::LuaVersion;

pub fn assert_no_errors(result: &ParseResult) {
    assert!(
        result.errors.is_empty(),
        "parse errors: {:?}",
        result.errors
    );
}

pub fn parse_luau(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Luau)
}
