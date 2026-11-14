use std::fs;
use std::path::Path;

use luck_lexer::{Lexer, lex};
use luck_testgen::collect_fixture_files;
use luck_token::{LuaVersion, TokenKind};

#[test]
fn next_token_streams_exactly_what_lex_materializes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let fixtures = collect_fixture_files(&root).expect("fixture tree is readable");
    assert!(!fixtures.is_empty(), "no fixtures under {}", root.display());
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
    for path in &fixtures {
        sources.push(fs::read_to_string(path).expect("fixture is readable"));
    }

    for version in LuaVersion::ALL {
        for source in &sources {
            let materialized = lex(source, version);
            let mut lexer = Lexer::new(source, version).expect("source fits u32 spans");
            let mut streamed = Vec::new();
            loop {
                let token = lexer.next_token();
                let is_eof = token.kind == TokenKind::Eof;
                streamed.push(token);
                if is_eof {
                    break;
                }
            }
            assert_eq!(streamed, materialized.tokens, "{version:?} {source:?}");
            let eof = streamed.last().expect("an EOF token");
            assert_eq!(&lexer.next_token(), eof, "EOF repeats: {source:?}");
            let (comments, errors) = lexer.finish();
            assert_eq!(comments, materialized.comments, "{version:?} {source:?}");
            assert_eq!(errors, materialized.errors, "{version:?} {source:?}");
        }
    }
}
