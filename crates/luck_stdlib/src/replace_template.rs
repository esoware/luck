/// `%1` through `%9` stand for `args[0]` through `args[8]`, and `%%` is a
/// literal percent.
///
/// A placeholder in a call's argument list expands verbatim,
/// and one before that list's `)` also forwards every later argument, so a
/// fix keeps the arguments the template doesn't spell out. Any other
/// placeholder is an operand, which gets parentheses unless it's a bare
/// operand, so the template's operator applies to the whole argument.
///
/// A missing argument expands to nothing; the caller decides whether the fix
/// is still useful.
///
/// Each argument is source as written, bytes that need not be UTF-8, and so
/// is the expansion.
pub fn expand_replace_template(template: &str, args: &[&[u8]]) -> Vec<u8> {
    let mut output = Vec::with_capacity(template.len());
    let mut remaining = template.as_bytes();
    while let Some(percent) = remaining.iter().position(|&b| b == b'%') {
        let (literal, from_percent) = remaining.split_at(percent);
        output.extend_from_slice(literal);
        let after_percent = &from_percent[1..];
        match after_percent.split_first() {
            Some((b'%', after_escape)) => {
                output.push(b'%');
                remaining = after_escape;
            }
            Some((&digit, after_digit)) if digit.is_ascii_digit() => {
                remaining = after_digit;
                let Some(from_argument) =
                    usize::from(digit - b'0').checked_sub(1).and_then(|i| args.get(i..))
                else {
                    continue;
                };
                let Some(&arg) = from_argument.first() else {
                    continue;
                };
                let after_placeholder = remaining.trim_ascii_start().first();
                let is_in_argument_list =
                    matches!(output.trim_ascii_end().last(), Some(b'(' | b','))
                        && matches!(after_placeholder, Some(b')' | b','));
                if is_in_argument_list && after_placeholder == Some(&b')') {
                    output.extend_from_slice(&from_argument.join(&b", "[..]));
                } else if is_in_argument_list || is_bare_operand(arg) {
                    output.extend_from_slice(arg);
                } else {
                    output.push(b'(');
                    output.extend_from_slice(arg);
                    output.push(b')');
                }
            }
            _ => {
                output.push(b'%');
                remaining = after_percent;
            }
        }
    }
    output.extend_from_slice(remaining);
    output
}

/// No operator can split one. Anything else gets grouped, which is harmless:
/// a parenthesized call only keeps its first value, and an operand takes one
/// value anyway.
fn is_bare_operand(arg: &[u8]) -> bool {
    !arg.windows(2).any(|pair| pair == b"..")
        && arg.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}
