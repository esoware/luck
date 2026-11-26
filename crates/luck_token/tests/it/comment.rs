use luck_token::CommentKind;

fn body(text: &str, kind: CommentKind) -> Option<&str> {
    kind.body(text.as_bytes()).map(|body| str::from_utf8(body).expect("UTF-8"))
}

#[test]
fn a_line_comment_s_body_follows_every_opening_dash() {
    assert_eq!(body("--- Greets someone.", CommentKind::Line), Some(" Greets someone."));
    assert_eq!(body("--", CommentKind::Line), Some(""));
}

#[test]
fn a_block_comment_s_body_ends_at_the_closer_of_its_own_level() {
    assert_eq!(body("--[[ see t[1]]]", CommentKind::SingleLineBlock), Some(" see t[1]"));
    assert_eq!(body("--[==[ a ]] b ]==]", CommentKind::SingleLineBlock), Some(" a ]] b "));
    assert_eq!(body("--[=[ cut off", CommentKind::MultiLineBlock), Some(" cut off"));
}

#[test]
fn a_shebang_has_no_body() {
    assert_eq!(body("#!/usr/bin/env luau", CommentKind::Shebang), None);
}
