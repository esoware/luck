use luck_token::{Comment, CommentKind, CommentPosition, Span, leading_hot_comments};

fn comment_at(source: &str, text: &str, kind: CommentKind) -> Comment {
    let start = source.find(text).unwrap_or_default();
    let span = Span::from_indices(start, start.saturating_add(text.len()));
    Comment {
        span,
        attached_to: span.end,
        kind,
        position: CommentPosition::Leading,
        is_preceded_by_newline: false,
        is_followed_by_newline: true,
    }
}

fn hot_comment_texts<'source>(comments: &[Comment], source: &'source str) -> Vec<&'source str> {
    leading_hot_comments(comments, source).map(|comment| comment.span.text(source)).collect()
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
    assert_eq!(hot_comment_texts(&comments, source), ["--!strict"]);
}

#[test]
fn a_hot_comment_after_code_is_not_leading() {
    let source = "\
--[[x]] local y = 1
--!strict
";
    let comments = [
        comment_at(source, "--[[x]]", CommentKind::SingleLineBlock),
        comment_at(source, "--!strict", CommentKind::Line),
    ];
    assert_eq!(hot_comment_texts(&comments, source), Vec::<&str>::new());
}

#[test]
fn a_hot_comment_line_inside_a_block_comment_is_not_a_comment() {
    let source = "\
--[[
--!strict
]]
print(1)
";
    let comments = [comment_at(
        source,
        "\
--[[
--!strict
]]",
        CommentKind::MultiLineBlock,
    )];
    assert_eq!(hot_comment_texts(&comments, source), Vec::<&str>::new());
}

#[test]
fn every_hot_comment_of_the_run_counts() {
    let source = "--!strict\n\n--!native\r\nlocal x\n";
    let comments = [
        comment_at(source, "--!strict", CommentKind::Line),
        comment_at(source, "--!native", CommentKind::Line),
    ];
    assert_eq!(hot_comment_texts(&comments, source), ["--!strict", "--!native"]);
}

#[test]
fn a_hot_comment_after_non_ascii_whitespace_is_not_leading() {
    let source = "\
\u{A0}--!strict
local x
";
    let comments = [comment_at(source, "--!strict", CommentKind::Line)];
    assert_eq!(
        hot_comment_texts(&comments, source),
        Vec::<&str>::new(),
        "Luau lexes U+00A0 as a token, so a hot comment after it comes after the first token"
    );
}
