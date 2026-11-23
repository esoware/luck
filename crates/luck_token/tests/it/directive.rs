use luck_token::{
    Comment, CommentContent, CommentKind, CommentPosition, DirectiveErrorKind, LintDirective,
    LintExtent, LintVerb, LuaVersion, Span,
};

fn comment(text: &str, kind: CommentKind) -> Comment {
    Comment {
        span: Span::from_indices(0, text.len()),
        attached_to: 0,
        kind,
        position: CommentPosition::Leading,
        is_followed_by_newline: true,
        content: CommentContent::of(text, kind, LuaVersion::Luau),
    }
}

fn content(text: &str) -> CommentContent {
    CommentContent::of(text, CommentKind::Line, LuaVersion::Luau)
}

fn extent(text: &str) -> Option<LintExtent> {
    LintDirective::parse(&comment(text, CommentKind::Line), text)
        .ok()
        .flatten()
        .map(|found| found.extent)
}

/// What's malformed in `text`'s directive, and the text it points at.
fn error(text: &str) -> Option<(DirectiveErrorKind, &str)> {
    let found = LintDirective::parse(&comment(text, CommentKind::Line), text).err()?;
    Some((found.kind, found.span.text(text)))
}

#[test]
fn reads_each_formatter_directive() {
    for (text, expected) in [
        ("-- luck: format off", CommentContent::FormatOff),
        ("--luck:format on", CommentContent::FormatOn),
        ("--- luck: ignore", CommentContent::FormatIgnore),
        ("-- luck: format ignore", CommentContent::FormatIgnore),
        ("-- luck: format off because generated", CommentContent::FormatOff),
        ("-- luck: ignore this one", CommentContent::FormatIgnore),
    ] {
        assert_eq!(content(text), expected, "{text:?}");
    }
}

#[test]
fn a_directive_word_ends_at_whitespace() {
    for text in [
        "-- luck: format offset",
        "-- luck: format one-liner",
        "-- luck: ignored",
        "-- luck: format ignored",
        "-- luck: ignore(x)",
        "-- luck: alow(x)",
        "-- luck:",
        "-- #luck: format off",
    ] {
        assert_eq!(content(text), CommentContent::InvalidDirective, "{text:?}");
    }
}

#[test]
fn reads_a_lint_directive_in_any_comment_form() {
    for (text, kind) in [
        ("-- luck: allow(x)", CommentKind::Line),
        ("-- #luck: deny(x)", CommentKind::Line),
        ("--[[ luck: warn(x) end ]]", CommentKind::SingleLineBlock),
        (
            "\
--[==[
luck: allow (x) start
]==]",
            CommentKind::MultiLineBlock,
        ),
        ("-- luck: allow x", CommentKind::Line),
    ] {
        assert_eq!(
            CommentContent::of(text, kind, LuaVersion::Lua54),
            CommentContent::LintDirective,
            "{text:?}"
        );
    }
}

#[test]
fn leaves_prose_plain() {
    for text in ["-- just a comment", "-- Luck: allow(x)", "--[ luck: allow(x)", "-- the luck: x"] {
        assert_eq!(content(text), CommentContent::Plain, "{text:?}");
    }
    assert_eq!(
        CommentContent::of("#!/usr/bin/env lua luck:", CommentKind::Shebang, LuaVersion::Lua54),
        CommentContent::Plain
    );
}

#[test]
fn a_hot_comment_is_luau_and_a_word_after_the_bang() {
    assert_eq!(content("--!strict"), CommentContent::HotComment);
    assert_eq!(content("--!optimize 2"), CommentContent::HotComment);
    for text in ["--!", "--! note", "--!\tnote", "---!strict"] {
        assert_eq!(content(text), CommentContent::Plain, "{text:?}");
    }
    assert_eq!(
        CommentContent::of("--!strict", CommentKind::Line, LuaVersion::Lua54),
        CommentContent::Plain
    );
    assert_eq!(
        CommentContent::of("--[[!strict]]", CommentKind::SingleLineBlock, LuaVersion::Luau),
        CommentContent::Plain
    );
}

#[test]
fn parses_the_verb_extent_and_rules() {
    let source = "--[=[ luck: warn( a, *,, b ) end ]=]";
    let Ok(Some(directive)) =
        LintDirective::parse(&comment(source, CommentKind::SingleLineBlock), source)
    else {
        panic!("a valid lint directive");
    };
    assert_eq!((directive.verb, directive.extent), (LintVerb::Warn, LintExtent::RegionEnd));
    let rules: Vec<&str> = directive.rules(source).map(|span| span.text(source)).collect();
    assert_eq!(rules, ["a", "*", "b"]);
    assert_eq!(extent("-- luck: allow(x)"), Some(LintExtent::Statement));
    assert_eq!(extent("-- luck: deny(x) start"), Some(LintExtent::RegionStart));
    assert_eq!(extent("-- #luck: allow(x)"), Some(LintExtent::File));
}

#[test]
fn a_comment_with_no_lint_directive_parses_to_none() {
    for text in ["-- prose", "-- luck: format off", "-- luck: ignore"] {
        assert_eq!(LintDirective::parse(&comment(text, CommentKind::Line), text), Ok(None));
    }
}

#[test]
fn rule_spans_are_absolute() {
    let source = "local x = 1 -- luck: allow(foo, bar)";
    let Ok(Some(directive)) = LintDirective::parse(
        &Comment { span: Span::new(12, 36), ..comment("", CommentKind::Line) },
        source,
    ) else {
        panic!("a valid lint directive");
    };
    let spans: Vec<Span> = directive.rules(source).collect();
    assert_eq!(spans, [Span::new(27, 30), Span::new(32, 35)]);
}

#[test]
fn says_what_is_malformed_at_the_part_at_fault() {
    for (text, kind, part) in [
        ("-- luck: allow(x) strat", DirectiveErrorKind::UnknownExtent(LintVerb::Allow), "strat"),
        ("-- #luck: allow(x) start", DirectiveErrorKind::RegionOnFileWide, "start"),
        ("-- luck: deny x", DirectiveErrorKind::MissingList(LintVerb::Deny), "deny"),
        ("-- luck: deny (x", DirectiveErrorKind::UnclosedList, "("),
        ("-- luck: warn( , )", DirectiveErrorKind::EmptyList(LintVerb::Warn), "( , )"),
        ("-- luck: alow(x)", DirectiveErrorKind::UnknownDirective, "alow(x)"),
        ("-- luck: format of", DirectiveErrorKind::UnknownFormatSetting, "of"),
        ("-- luck: format", DirectiveErrorKind::UnknownFormatSetting, "format"),
        ("-- #luck: ignore", DirectiveErrorKind::NotLintVerb, "ignore"),
        ("-- luck: ", DirectiveErrorKind::Empty, "-- luck: "),
    ] {
        assert_eq!(error(text), Some((kind, part)), "{text:?}");
    }
}
