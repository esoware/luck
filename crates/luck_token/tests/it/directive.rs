use luck_token::{
    Comment, CommentContent, CommentKind, CommentPosition, DirectiveErrorKind, LintDirective,
    LintExtent, LintVerb, Span,
};

fn comment(text: &[u8], kind: CommentKind) -> Comment {
    Comment {
        span: Span::from_indices(0, text.len()),
        attached_to: 0,
        kind,
        position: CommentPosition::Leading,
        is_followed_by_newline: true,
        content: CommentContent::of(text, kind),
    }
}

fn content(text: &str) -> CommentContent {
    CommentContent::of(text.as_bytes(), CommentKind::Line)
}

fn extent(text: &str) -> Option<LintExtent> {
    LintDirective::parse(&comment(text.as_bytes(), CommentKind::Line), text.as_bytes())
        .ok()
        .flatten()
        .map(|found| found.extent)
}

/// What's malformed in `text`'s directive, and the text it points at.
fn error(text: &[u8]) -> Option<(DirectiveErrorKind, &[u8])> {
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
            CommentContent::of(text.as_bytes(), kind),
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
        CommentContent::of(b"#!/usr/bin/env lua luck:", CommentKind::Shebang),
        CommentContent::Plain
    );
}

#[test]
fn a_hot_comment_is_a_line_comment_with_a_word_after_the_bang() {
    assert_eq!(content("--!strict"), CommentContent::HotComment);
    assert_eq!(content("--!optimize 2"), CommentContent::HotComment);
    for text in ["--!", "--! note", "--!\tnote", "---!strict"] {
        assert_eq!(content(text), CommentContent::Plain, "{text:?}");
    }
    assert_eq!(
        CommentContent::of(b"--[[!strict]]", CommentKind::SingleLineBlock),
        CommentContent::Plain
    );
}

#[test]
fn parses_the_verb_extent_and_rules() {
    let source = b"--[=[ luck: warn( a, *,, b ) end ]=]";
    let Ok(Some(directive)) =
        LintDirective::parse(&comment(source, CommentKind::SingleLineBlock), source)
    else {
        panic!("a valid lint directive");
    };
    assert_eq!((directive.verb, directive.extent), (LintVerb::Warn, LintExtent::RegionEnd));
    let rules: Vec<&[u8]> = directive.rules(source).map(|span| span.text(source)).collect();
    assert_eq!(rules, [b"a", b"*", b"b"]);
    assert_eq!(extent("-- luck: allow(x)"), Some(LintExtent::Statement));
    assert_eq!(extent("-- luck: deny(x) start"), Some(LintExtent::RegionStart));
    assert_eq!(extent("-- #luck: allow(x)"), Some(LintExtent::File));
}

#[test]
fn a_comment_with_no_lint_directive_parses_to_none() {
    for text in ["-- prose", "-- luck: format off", "-- luck: ignore"] {
        let bytes = text.as_bytes();
        assert_eq!(LintDirective::parse(&comment(bytes, CommentKind::Line), bytes), Ok(None));
    }
}

#[test]
fn rule_spans_are_absolute() {
    let source = b"local x = 1 -- luck: allow(foo, bar)";
    let Ok(Some(directive)) = LintDirective::parse(
        &Comment { span: Span::new(12, 36), ..comment(b"", CommentKind::Line) },
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
        assert_eq!(error(text.as_bytes()), Some((kind, part.as_bytes())), "{text:?}");
    }
}

#[test]
fn reads_a_directive_in_a_comment_that_is_not_utf8() {
    for (text, expected) in [
        (&b"-- luck: format off (g\xE9n\xE9r\xE9)"[..], CommentContent::FormatOff),
        (b"-- luck: allow(x) start", CommentContent::LintDirective),
        (b"-- luck: allow(x) \xE9", CommentContent::LintDirective),
        (b"-- luck: \xE9", CommentContent::InvalidDirective),
        (b"-- \xE9 luck: allow(x)", CommentContent::Plain),
    ] {
        assert_eq!(CommentContent::of(text, CommentKind::Line), expected);
    }
    assert_eq!(
        error(b"-- luck: allow(x) \xE9t\xE9"),
        Some((DirectiveErrorKind::UnknownExtent(LintVerb::Allow), &b"\xE9t\xE9"[..]))
    );
}
