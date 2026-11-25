//! A comment can sit in any gap between two tokens, and formatting has to
//! keep it there, near enough, without changing the tree or losing it.

use luck_allocator::Allocator;
use luck_formatter::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatOptions, SpaceAfterFunction,
    format,
};
use luck_parser::ParseResult;
use luck_testgen::{generate_full_grammar, generate_runtime_safe, luau_fixtures, tree_shape};
use luck_token::{Span, TokenKind, index_to_offset};

const SEED_COUNT: u64 = 8;
const STATEMENT_BUDGET: usize = 25;

/// How the comment put in each gap sits.
#[derive(Debug, Clone, Copy)]
enum Shape {
    Inline,
    EndingTheLine,
    OnItsOwnLine,
    InlineThenEndingTheLine,
    SpanningLines,
    BetweenBlankLines,
}

impl Shape {
    const ALL: [Self; 6] = [
        Self::Inline,
        Self::EndingTheLine,
        Self::OnItsOwnLine,
        Self::InlineThenEndingTheLine,
        Self::SpanningLines,
        Self::BetweenBlankLines,
    ];

    fn text(self, index: usize) -> String {
        match self {
            Self::Inline => format!(" --[[c{index}]] "),
            Self::EndingTheLine => format!(" -- c{index}\n"),
            Self::OnItsOwnLine => format!("\n--[[c{index}]]\n"),
            Self::InlineThenEndingTheLine => format!(" --[[c{index}]] -- d{index}\n"),
            Self::SpanningLines => format!(" --[[c{index}\n]] "),
            Self::BetweenBlankLines => format!("\n\n-- c{index}\n\n"),
        }
    }

    const fn breaks_the_line(self) -> bool {
        match self {
            Self::Inline => false,
            Self::EndingTheLine
            | Self::OnItsOwnLine
            | Self::InlineThenEndingTheLine
            | Self::SpanningLines
            | Self::BetweenBlankLines => true,
        }
    }
}

/// `source` with a comment of `shape` after every token.
///
/// That's but where no comment is allowed, after an attribute's `@`, and
/// where a line break doesn't parse, before a call's `(`.
fn with_a_comment_in_every_gap(source: &str, shape: Shape) -> String {
    let allocator = Allocator::new();
    let parse = luck_parser::parse_with_tokens(&allocator, source.as_bytes());
    let mut commented = String::new();
    let mut copied = 0;
    for (index, pair) in parse.tokens.windows(2).enumerate() {
        let [token, next] = pair else {
            continue;
        };
        if token.kind == TokenKind::At
            || (shape.breaks_the_line() && next.kind == TokenKind::LeftParen)
        {
            continue;
        }
        commented.push_str(
            str::from_utf8(Span::new(copied, token.span.end).text(source.as_bytes())).unwrap(),
        );
        commented.push_str(&shape.text(index));
        copied = token.span.end;
    }
    let rest = Span::new(copied, index_to_offset(source.len())).text(source.as_bytes());
    commented.push_str(str::from_utf8(rest).unwrap());
    commented
}

/// The text of every comment in `parse`, as the formatter prints it, sorted.
fn comment_texts(parse: &ParseResult<'_>) -> Vec<String> {
    let mut texts: Vec<String> = parse
        .comments
        .iter()
        .map(|comment| {
            let text = String::from_utf8_lossy(comment.span.text(parse.source));
            text.replace("\r\n", "\n").trim_end().to_owned()
        })
        .collect();
    texts.sort_unstable();
    texts
}

/// The lines around the first one `left` and `right` differ in.
fn first_difference(left: &str, right: &str) -> String {
    let line = left.lines().zip(right.lines()).take_while(|(a, b)| a == b).count();
    let around = |text: &str| -> Vec<String> {
        text.lines().skip(line.saturating_sub(3)).take(6).map(str::to_owned).collect()
    };
    format!("{:#?}\nthen\n{:#?}", around(left), around(right))
}

/// The default layout and the ones furthest from it.
fn option_sets() -> [FormatOptions; 3] {
    let defaults = FormatOptions::default();
    [
        defaults,
        FormatOptions {
            line_width: 1,
            call_parentheses: CallParentheses::None,
            collapse_simple_statement: CollapseSimpleStatement::ConditionalOnly,
            magic_trailing_comma: false,
            ..defaults
        },
        FormatOptions {
            line_width: 40,
            call_parentheses: CallParentheses::Input,
            block_newline_gaps: BlockNewlineGaps::Preserve,
            space_after_function_names: SpaceAfterFunction::Always,
            ..defaults
        },
    ]
}

fn assert_comments_keep_the_tree(label: &str, source: &str) {
    let allocator = Allocator::new();
    let original = luck_parser::parse(&allocator, source.as_bytes());
    let expected_shape = tree_shape(&original.chunk);
    for shape in Shape::ALL {
        let commented = with_a_comment_in_every_gap(source, shape);
        let commented_parse = luck_parser::parse(&allocator, commented.as_bytes());
        assert!(
            commented_parse.errors.is_empty(),
            "{label}, {shape:?}: the commented source parses: {:?}\n{commented}",
            commented_parse.errors
        );
        let written = comment_texts(&commented_parse);
        for (set, options) in option_sets().iter().enumerate() {
            let case = format!("{label}, {shape:?}, option set {set}");
            let output =
                String::from_utf8(format(&allocator, commented.as_bytes(), options).output)
                    .unwrap();
            let again =
                String::from_utf8(format(&allocator, output.as_bytes(), options).output).unwrap();
            assert!(
                again == output,
                "{case}: not idempotent:\n{}",
                first_difference(&output, &again)
            );
            let formatted = luck_parser::parse(&allocator, output.as_bytes());
            assert!(formatted.errors.is_empty(), "{case}: {:?}\n{output}", formatted.errors);
            assert_eq!(
                tree_shape(&formatted.chunk),
                expected_shape,
                "{case}: formatting changed the tree:\n{output}"
            );
            let printed = comment_texts(&formatted);
            if printed != written {
                let lost: Vec<&String> =
                    written.iter().filter(|text| !printed.contains(text)).collect();
                let gained: Vec<&String> =
                    printed.iter().filter(|text| !written.contains(text)).collect();
                panic!("{case}: lost {lost:?} and gained {gained:?}:\n{output}");
            }
        }
    }
}

#[test]
fn a_comment_in_any_gap_of_a_fixture_keeps_its_tree() {
    let fixtures: Vec<_> =
        luau_fixtures().into_iter().filter(|fixture| !fixture.is_error).collect();
    assert!(!fixtures.is_empty(), "no fixture outside `errors/`");
    for fixture in fixtures {
        let label = fixture.relative_path.display().to_string();
        assert_comments_keep_the_tree(&label, &fixture.source);
    }
}

#[test]
fn a_comment_in_any_gap_of_a_generated_program_keeps_its_tree() {
    for seed in 0..SEED_COUNT {
        let full = generate_full_grammar(seed, STATEMENT_BUDGET);
        assert_comments_keep_the_tree(&format!("full seed {seed}"), &full);
        let runtime = generate_runtime_safe(seed, STATEMENT_BUDGET);
        assert_comments_keep_the_tree(&format!("runtime seed {seed}"), &runtime);
    }
}
