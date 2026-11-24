use std::fs;

use luck_lexer::{Lexer, lex};
use luck_testgen::fixture_files;
use luck_token::{LuaVersion, TokenKind};

#[test]
fn a_lexer_is_send_and_sync() {
    fn assert_send_sync<T>()
    where
        T: Send + Sync,
    {
    }
    assert_send_sync::<Lexer<'static>>();
}

#[test]
fn next_token_streams_exactly_what_lex_materializes() {
    let mut sources = [
        "",
        "\
local x = 1 -- trailing
-- leading
return x",
        "#!/usr/bin/env lua\r\n--[==[ block\r\n ]==] print'a' --[[ tail ]]",
        "\
\u{FEFF}x = [[long
string]] .. \"esc\\n\" -- end",
        "\
x = \"unterminated
y = 0x --[[ open",
        "`plain` `a{b}c{ {d} }e` `{`",
        "local t = { ~ @ $ caf\u{e9} }",
    ]
    .map(String::from)
    .to_vec();
    for path in fixture_files() {
        sources.push(fs::read_to_string(path).expect("fixture is readable"));
    }

    for version in LuaVersion::ALL {
        for source in &sources {
            let materialized = lex(source.as_bytes(), version);
            let mut lexer = Lexer::new(source.as_bytes(), version).expect("source fits u32 spans");
            let mut streamed = Vec::new();
            loop {
                let token = lexer.next_token();
                let is_eof = token.kind == TokenKind::Eof;
                streamed.push(token);
                if is_eof {
                    break;
                }
            }
            let eof = *streamed.last().expect("an EOF token");
            assert_eq!(lexer.next_token(), eof, "EOF repeats: {source:?}");
            assert_eq!(lexer.finish(streamed), materialized, "{version:?} {source:?}");
        }
    }
}
