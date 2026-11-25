use std::fs;

use luck_lexer::{LexResult, Lexer, lex};
use luck_simd::DynCpu;
use luck_testgen::{fixture_files, generate_full_grammar};
use luck_token::TokenKind;

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
        "#!/usr/bin/env luau\r\n--[==[ block\r\n ]==] print'a' --[[ tail ]]",
        "\
x = [[long
string]] .. \"esc\\n\" -- end",
        "\u{FEFF}x = 1",
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

    for source in &sources {
        let materialized = lex(source.as_bytes());
        let mut lexer = Lexer::new(source.as_bytes()).expect("source fits u32 spans");
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
        assert_eq!(lexer.finish(streamed), materialized, "{source:?}");
    }
}

fn stream(source: &[u8], cpu: DynCpu) -> LexResult {
    let mut lexer = Lexer::with_cpu(source, cpu).expect("source fits u32 spans");
    let mut tokens = Vec::new();
    loop {
        let token = lexer.next_token();
        tokens.push(token);
        if token.kind == TokenKind::Eof {
            return lexer.finish(tokens);
        }
    }
}

/// Runs of one byte, a run of whitespace or a name as often as not longer
/// than a 64-byte window, drawn from the bytes whose scans a SIMD feature set
/// changes, beside the bytes that end them.
fn byte_soup(seed: u64) -> Vec<u8> {
    const BYTES: &[u8] = b" \t\n\r\x0B\x0C\x00azZ_09-[]=.\"'`{}\\\x80\xC3\xA9\xFF(~";
    let mut state = seed.max(1);
    let mut draw = |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % bound as u64).expect("below `bound`")
    };
    let mut soup = Vec::new();
    for _ in 0..draw(40) {
        let byte = BYTES[draw(BYTES.len())];
        soup.extend(std::iter::repeat_n(byte, 1 + draw(100)));
    }
    soup
}

/// Pieces of tokens and the bytes between them, side by side with no space,
/// so every boundary a window's bit tricks draw falls next to every other.
fn token_soup(seed: u64) -> Vec<u8> {
    const PIECES: &[&[u8]] = &[
        b"a",
        b"_x1",
        b"end",
        b"local",
        b"function",
        b"type",
        b"continue",
        b"averyveryverylongnamethatrunspastasixtyfourbytewindowendforsure_0123456789",
        b"1",
        b"123",
        b"0x1F",
        b"1.5",
        b".5",
        b"5.",
        b"1e5",
        b"1_000",
        b"0b1",
        b"5i",
        b"1..2",
        b".",
        b"..",
        b"...",
        b"..=",
        b"=",
        b"==",
        b"~=",
        b"~",
        b"<",
        b"<=",
        b"<<",
        b">",
        b">>",
        b":",
        b"::",
        b"-",
        b"->",
        b"-=",
        b"+",
        b"+=",
        b"*",
        b"/",
        b"//",
        b"//=",
        b"%",
        b"^",
        b"#",
        b"&",
        b"|",
        b"@",
        b"?",
        b"(",
        b")",
        b"{",
        b"}",
        b"[",
        b"]",
        b",",
        b";",
        b"--c\n",
        b"--[[c]]",
        b"[[s]]",
        b"[=[s]=]",
        b"[=",
        b"\"s\"",
        b"'s'",
        b"\"\\n\"",
        b"`i{x}`",
        b"`",
        b" ",
        b"  ",
        b"\t",
        b"\n",
        b"\r",
        b"\r\n",
        b"\n\r",
        b"\x0B",
        b"\x0C",
        b"\x00",
        b"$",
        b"!",
        b"\\",
        b"\xC3\xA9",
        b"\xFF",
    ];
    let mut state = seed.max(1);
    let mut draw = |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % bound as u64).expect("below `bound`")
    };
    let mut soup = Vec::new();
    for _ in 0..draw(400) {
        soup.extend_from_slice(PIECES[draw(PIECES.len())]);
    }
    soup
}

#[test]
fn every_feature_set_lexes_exactly_as_the_best_one_does() {
    let mut sources: Vec<Vec<u8>> =
        fixture_files().into_iter().map(|path| fs::read(path).expect("fixture")).collect();
    for seed in 0..200 {
        sources.push(byte_soup(seed));
    }
    for seed in 0..2000 {
        sources.push(token_soup(seed));
    }
    for seed in 0..24 {
        sources.push(generate_full_grammar(seed, 200).into_bytes());
    }
    let cpus = DynCpu::available();
    for source in &sources {
        let best = stream(source, cpus[0]);
        for &cpu in &cpus[1..] {
            let lexed = stream(source, cpu);
            assert_eq!(lexed, best, "{cpu:?} {:?}", source.escape_ascii());
        }
    }
}
