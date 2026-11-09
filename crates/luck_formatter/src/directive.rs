use luck_token::{Comment, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Directive {
    FormatOff,
    FormatOn,
    /// `luck: ignore` or `luck: format ignore`, which keeps the next
    /// statement as written.
    Ignore,
}

impl Directive {
    pub(crate) fn parse(comment_text: &str) -> Option<Self> {
        let body = comment_text.trim_start_matches('-').trim();
        if opens_with_words(body, "luck: ignore") || opens_with_words(body, "luck: format ignore") {
            Some(Self::Ignore)
        } else if opens_with_words(body, "luck: format off") {
            Some(Self::FormatOff)
        } else if opens_with_words(body, "luck: format on") {
            Some(Self::FormatOn)
        } else {
            None
        }
    }
}

/// Whether `body` starts with `words`, ending at a word boundary, so
/// `luck: format off` doesn't also match `luck: format offset`.
fn opens_with_words(body: &str, words: &str) -> bool {
    body.strip_prefix(words)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// Each `off` comment's span is paired with the span of the `on` that closes
/// its region, or `None` when the region runs to end of file. An `off` inside
/// an open region changes nothing.
pub(crate) fn format_off_regions(comments: &[Comment], source: &str) -> Vec<(Span, Option<Span>)> {
    let mut regions = Vec::new();
    let mut unclosed_off: Option<Span> = None;
    for comment in comments {
        match Directive::parse(comment.span.text(source)) {
            Some(Directive::FormatOff) => {
                unclosed_off.get_or_insert(comment.span);
            }
            Some(Directive::FormatOn) => {
                if let Some(off) = unclosed_off.take() {
                    regions.push((off, Some(comment.span)));
                }
            }
            Some(Directive::Ignore) | None => {}
        }
    }
    if let Some(off) = unclosed_off {
        regions.push((off, None));
    }
    regions
}
