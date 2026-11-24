use luck_token::{
    Comment, CommentContent, CommentKind, CommentPosition, LuaVersion, Span, index_to_offset,
    leading_hot_comments,
};

fn comment_at(source: &str, text: &str, kind: CommentKind) -> Comment {
    let start = source.find(text).expect("the comment is in the source");
    let span = Span::from_indices(start, start + text.len());
    Comment {
        span,
        attached_to: span.end,
        kind,
        position: CommentPosition::Leading,
        is_followed_by_newline: true,
        content: CommentContent::of(text.as_bytes(), kind, LuaVersion::Luau),
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
