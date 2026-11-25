use luck_token::{
    Comment, CommentContent, CommentKind, CommentPosition, Span, chunk_header, has_shebang,
    index_to_offset, leading_hot_comments,
};

fn span_of(source: &str, text: &str) -> Span {
    let start = source.find(text).expect("the text is in the source");
    Span::from_indices(start, start + text.len())
}

fn comment_at(source: &str, text: &str, kind: CommentKind) -> Comment {
    let span = span_of(source, text);
    Comment {
        span,
        attached_to: span.end,
        kind,
        position: CommentPosition::Leading,
        is_followed_by_newline: true,
        content: CommentContent::of(text.as_bytes(), kind),
    }
}

fn hot_comment_texts<'source>(
    comments: &[Comment],
    source: &'source str,
    first_token: &str,
) -> Vec<&'source str> {
    let first_token_start =
        index_to_offset(source.find(first_token).expect("the token is in the source"));
    leading_hot_comments(comments, first_token_start)
        .map(|comment| str::from_utf8(comment.span.text(source.as_bytes())).expect("UTF-8"))
        .collect()
}

#[test]
fn has_shebang_holds_only_for_a_hash_bang_line_longer_than_its_two_bytes() {
    assert!(has_shebang(b"#!/usr/bin/env luau\nprint(1)"));
    assert!(has_shebang(b"#!\n"));
    assert!(!has_shebang(b"print(1)"));
    assert!(!has_shebang(b""));
    assert!(!has_shebang(b"#!"));
    assert!(!has_shebang(b"# a note\nprint(1)"));
    assert!(!has_shebang("\u{FEFF}#!/usr/bin/env luau".as_bytes()));
}

#[test]
fn collects_hot_comments_across_other_comments() {
    let source = "\
#!/usr/bin/env luau
--[[
block
]]
-- note
--!strict
print(1)
";
    let comments = [
        comment_at(source, "#!/usr/bin/env luau", CommentKind::Shebang),
        comment_at(
            source,
            "\
--[[
block
]]",
            CommentKind::MultiLineBlock,
        ),
        comment_at(source, "-- note", CommentKind::Line),
        comment_at(source, "--!strict", CommentKind::Line),
    ];
    assert_eq!(hot_comment_texts(&comments, source, "print"), ["--!strict"]);
}

#[test]
fn a_hot_comment_after_the_first_token_is_not_leading() {
    let source = "\
--[[x]] local y = 1
--!strict
";
    let comments = [
        comment_at(source, "--[[x]]", CommentKind::SingleLineBlock),
        comment_at(source, "--!strict", CommentKind::Line),
    ];
    assert_eq!(hot_comment_texts(&comments, source, "local"), Vec::<&str>::new());
}

#[test]
fn every_hot_comment_of_the_run_counts_and_prose_does_not() {
    let source = "--!strict\n--! note\n--!native\r\nlocal x\n";
    let comments = [
        comment_at(source, "--!strict", CommentKind::Line),
        comment_at(source, "--! note", CommentKind::Line),
        comment_at(source, "--!native", CommentKind::Line),
    ];
    assert_eq!(hot_comment_texts(&comments, source, "local"), ["--!strict", "--!native"]);
}

/// A CR left at the end of the shebang would pair with the header's LF into
/// one CRLF, and whitespace after a hot comment is no part of it.
#[test]
fn chunk_header_puts_the_shebang_then_each_hot_comment_on_a_line_of_its_own() {
    let source = "#!/usr/bin/env luau\r\r\n--!strict \t\r\n--!native\nlocal x = 1\n";
    let shebang = span_of(source, "#!/usr/bin/env luau\r");
    let hot_comments = [span_of(source, "--!strict \t"), span_of(source, "--!native")];
    assert_eq!(
        chunk_header(source.as_bytes(), Some(shebang), hot_comments),
        b"#!/usr/bin/env luau\n--!strict\n--!native\n"
    );
    assert_eq!(chunk_header(source.as_bytes(), None, []), b"");
}
