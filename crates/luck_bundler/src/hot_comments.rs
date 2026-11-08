//! Luau hot comments, which take effect only before any code in a chunk.

/// Each `--!` hot comment in the comment run that opens `source`, with its
/// byte offset. The text is the line as `str::lines` yields it, without
/// leading whitespace.
pub(crate) fn leading_hot_comments(source: &str) -> impl Iterator<Item = (usize, &str)> {
    let line_starts = source.split_inclusive('\n').scan(0usize, |next_start, line| {
        let start = *next_start;
        *next_start = next_start.saturating_add(line.len());
        Some(start)
    });
    line_starts
        .zip(source.lines())
        .map(|(line_start, line)| {
            let trimmed = line.trim_start();
            let indent = line.len().saturating_sub(trimmed.len());
            (line_start.saturating_add(indent), trimmed)
        })
        .filter(|(_, trimmed)| !trimmed.is_empty())
        .take_while(|(_, trimmed)| trimmed.starts_with("--"))
        .filter(|(_, trimmed)| trimmed.starts_with("--!"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_comments_are_read_from_the_leading_comment_run_only() {
        let source = "--!strict\r\n\n  -- note\n\t--!native\nlocal x = 1\n--!optimize 2\n";
        let hot_comments: Vec<(usize, &str)> = leading_hot_comments(source).collect();
        assert_eq!(hot_comments, vec![(0, "--!strict"), (23, "--!native")]);
    }

    #[test]
    fn source_opening_with_code_has_no_hot_comments() {
        assert_eq!(leading_hot_comments("print(1)\n--!strict\n").count(), 0);
        assert_eq!(leading_hot_comments("").count(), 0);
    }
}
