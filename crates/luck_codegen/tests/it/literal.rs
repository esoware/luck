use luck_allocator::Allocator;
use luck_ast::{Chunk, SpanCounter, Synth};
use luck_codegen::LiteralSpelling;
use luck_token::LuaVersion;

use crate::common::{assert_compacts_to, assert_reprints, assert_spells};

/// A string as written keeps the bytes that aren't UTF-8, and one built from
/// its value is spelled in UTF-8.
#[test]
fn a_string_that_is_not_utf8_prints_as_written_or_escaped_from_its_value() {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, b"return 'caf\xE9', [[\xFF]]", LuaVersion::Lua51);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(
        luck_codegen::compact(&parsed.chunk, LiteralSpelling::AsWritten),
        b"return'caf\xE9',[[\xFF]]"
    );
    assert_reprints(&parsed.chunk);
    let spans = SpanCounter::new();
    let synth = Synth::new(&allocator, &spans);
    let built = synth.block([synth.return_([synth.string_bytes(b"caf\xE9")])]);
    let chunk = Chunk::new(LuaVersion::Lua51, built);
    assert_eq!(luck_codegen::compact(&chunk, LiteralSpelling::AsWritten), br#"return"caf\233""#);
}

#[test]
fn each_literal_prints_as_written() {
    for (source, expected, version) in [
        (r#"x = "\65\066" .. [[hello]]"#, r#"x="\65\066"..[[hello]]"#, LuaVersion::Lua54),
        ("x = 0x10, 1.50, 0.5, 1e2, 007, 1.0", "x=0x10,1.50,0.5,1e2,007,1.0", LuaVersion::Lua54),
        ("x = 1_000, 0b11, 0xFFi, 1_0i", "x=1_000,0b11,0xFFi,1_0i", LuaVersion::Luau),
        (r"x = `a\x41{b}\u{42}`", r"x=`a\x41{b}\u{42}`", LuaVersion::Luau),
    ] {
        assert_compacts_to(source, expected, version);
    }
}

#[test]
fn a_literal_no_spelling_beats_keeps_its_own_however_long() {
    let content = format!("\u{1A}\"'\\{}", "a".repeat(200_000));
    let source = format!("return [[{content}]]");
    let expected = format!("return[[{content}]]");
    assert_spells(&source, &expected, LuaVersion::Lua54, LiteralSpelling::Shortest);
}

#[test]
fn each_literal_takes_the_shortest_spelling_of_its_value() {
    let (lua51, lua54, luau) = (LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau);
    for (source, expected, version) in [
        (r#"x = "\65\066" .. [[hello]] .. 'a\'b'"#, r#"x="AB".."hello".."a'b""#, lua54),
        (r#"x = "say \"hi\"""#, r#"x='say "hi"'"#, lua54),
        ("x = \"a\\nb\\nc\\nd\"", "x=[[a\nb\nc\nd]]", lua54),
        ("x = 'a\tb'", "x='a\tb'", lua54),
        ("x = 0x10, 1.50, 0.5, 1e2, 007", "x=16,1.5,.5,1e2,7", lua54),
        ("x = 1.0, 1000, 100.0", "x=1.,1000,1e2", lua54),
        ("x = 1.0, 1000, 0x10, 1_000", "x=1,1e3,16,1e3", luau),
        ("x = 0x1p4, 0x.8", "x=16,.5", LuaVersion::Lua52),
        // A spelling as long as the shortest stays.
        ("x = 1e999, 9007199254740993", "x=1e999,9007199254740993", lua51),
        ("x = 1e9999, 100000000000000000000", "x=1e999,1e20", lua51),
        ("x = 0xFFi, 0b11i, 1_0i", "x=255i,3i,10i", luau),
        (r"x = `a\x41{b}\u{42}`", "x=`aA{b}B`", luau),
    ] {
        assert_spells(source, expected, version, LiteralSpelling::Shortest);
    }
}

#[test]
fn a_literal_built_in_code_prints_from_its_value() {
    let allocator = Allocator::new();
    let spans = SpanCounter::new();
    for (version, expected) in [
        (LuaVersion::Lua54, r#"return 3.,-1,'\255"'"#),
        (LuaVersion::Lua51, r#"return 3,-1,'\255"'"#),
    ] {
        let synth = Synth::new(&allocator, &spans).with_version(version);
        let values = [synth.number_f64(3.0), synth.number_i64(-1), synth.string_bytes(b"\xFF\"")];
        let chunk = Chunk::new(version, synth.block([synth.return_(values)]));
        for spelling in [LiteralSpelling::AsWritten, LiteralSpelling::Shortest] {
            let output = luck_codegen::compact(&chunk, spelling);
            assert_eq!(output, expected.as_bytes(), "{version:?} {spelling:?}");
        }
    }
}
