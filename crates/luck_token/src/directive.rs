//! Comments that tell a tool what to do: Luau's `--!` hot comments, and luck's
//! directives.
//!
//! A directive is a comment that opens with `luck:`, past its delimiters and
//! any whitespace, or with `#luck:` to cover the whole file, then one of:
//!
//! - `format off` and `format on`, which bound a region the formatter copies
//!   as written, or `ignore` or `format ignore`, which keeps the next statement
//!   as written. Words after one are a note to the reader.
//! - `allow(...)`, `deny(...)`, or `warn(...)` with a list of rule names, or
//!   `*` for every rule, then `start` or `end` to open or close a region.
//!   Nothing else may follow, so a misspelled `start` can't quietly shrink a
//!   region to one statement. Only these may follow `#luck:`.

use crate::{Comment, CommentKind, LuaVersion, Span, is_lua_whitespace};

/// What a comment tells a tool, which the lexer reads once from its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentContent {
    /// Prose, which no tool reads.
    Plain,
    /// A Luau `--!` hot comment, such as `--!strict`. Luau keeps `--!` alone,
    /// or before a space or a tab, for prose.
    HotComment,
    FormatOff,
    FormatOn,
    /// `ignore` or `format ignore`.
    FormatIgnore,
    /// `allow`, `deny`, or `warn`, whose list [`LintDirective::parse`] reads.
    LintDirective,
    /// A `luck:` comment that's no directive, which [`LintDirective::parse`]
    /// says why of.
    InvalidDirective,
}

impl CommentContent {
    /// What a comment of `kind` says in `version`, from its `text`, delimiters
    /// included.
    #[must_use]
    pub fn of(text: &str, kind: CommentKind, version: LuaVersion) -> Self {
        if kind == CommentKind::Line
            && version.has_hot_comments()
            && let Some(word) = text.strip_prefix("--!")
        {
            return if word.starts_with(|c: char| c != ' ' && c != '\t') {
                Self::HotComment
            } else {
                Self::Plain
            };
        }
        let Some((command, is_file_wide)) = command(text, kind) else {
            return Self::Plain;
        };
        match read_command(command, is_file_wide) {
            Ok(Command::FormatOff) => Self::FormatOff,
            Ok(Command::FormatOn) => Self::FormatOn,
            Ok(Command::FormatIgnore) => Self::FormatIgnore,
            Ok(Command::Lint { .. }) => Self::LintDirective,
            Err(_) => Self::InvalidDirective,
        }
    }
}

/// What a lint directive does to what its rules report: `allow` drops it, and
/// `deny` and `warn` make it an error or a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintVerb {
    Allow,
    Deny,
    Warn,
}

impl LintVerb {
    /// The verb as a directive spells it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Warn => "warn",
        }
    }
}

/// What's malformed in a `luck:` comment, at the part at fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectiveError {
    pub span: Span,
    pub kind: DirectiveErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveErrorKind {
    /// Nothing follows `luck:`, and the span is the whole comment.
    Empty,
    /// `#luck:` before a word other than a lint verb.
    NotLintVerb,
    /// A word that names no directive.
    UnknownDirective,
    /// `format` before a word other than `off`, `on`, or `ignore`, or before
    /// none, when the span is `format` itself.
    UnknownFormatSetting,
    /// A lint verb with no `(` after it, at the verb.
    MissingList(LintVerb),
    /// A `(` with no `)`, at the `(`.
    UnclosedList,
    /// A list that names no rule, at the list and its parentheses.
    EmptyList(LintVerb),
    /// `start` or `end` after the list of a `#luck:` directive, which covers
    /// the whole file.
    RegionOnFileWide,
    /// A word after the list other than `start` or `end`.
    UnknownExtent(LintVerb),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintExtent {
    /// `#luck:`.
    File,
    /// No `start` or `end`.
    Statement,
    /// `start`, which runs to the `end` with the same verb and rule, or to
    /// the end of the file.
    RegionStart,
    /// `end`, which closes the latest open region with the same verb and
    /// rule.
    RegionEnd,
}

/// An `allow`, `deny`, or `warn` directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LintDirective {
    pub verb: LintVerb,
    pub extent: LintExtent,
    /// The text between the parentheses.
    pub list: Span,
}

impl LintDirective {
    /// The lint directive in `comment`, or `None` for a comment that holds
    /// none, which the lexer reads as no [`CommentContent::LintDirective`] or
    /// [`CommentContent::InvalidDirective`].
    ///
    /// # Errors
    ///
    /// Returns why a `luck:` comment is no directive, or what's malformed in
    /// one, at the part at fault.
    pub fn parse(comment: &Comment, source: &str) -> Result<Option<Self>, DirectiveError> {
        let text = comment.span.text(source);
        let start = comment.span.start;
        let error = |part: &str, kind| DirectiveError { span: span_in(text, start, part), kind };
        let Some((command, is_file_wide)) = command(text, comment.kind) else {
            return Ok(None);
        };
        let (verb, verb_word, after_verb) = match read_command(command, is_file_wide) {
            Ok(Command::Lint { verb, word, rest }) => (verb, word, rest),
            Ok(Command::FormatOff | Command::FormatOn | Command::FormatIgnore) => return Ok(None),
            Err(invalid) => {
                let kind = invalid.kind();
                return Err(invalid
                    .word()
                    .map_or(DirectiveError { span: comment.span, kind }, |word| {
                        error(word, kind)
                    }));
            }
        };
        let Some(inside) = trim_start(after_verb).strip_prefix('(') else {
            return Err(error(verb_word, DirectiveErrorKind::MissingList(verb)));
        };
        let Some((list, after_list)) = inside.split_once(')') else {
            let open =
                after_verb.get(..after_verb.len().saturating_sub(inside.len())).unwrap_or_default();
            return Err(error(trim_start(open), DirectiveErrorKind::UnclosedList));
        };
        let span = span_in(text, start, list);
        if names(list).next().is_none() {
            let parenthesized = Span::new(span.start.saturating_sub(1), span.end.saturating_add(1));
            return Err(DirectiveError {
                span: parenthesized,
                kind: DirectiveErrorKind::EmptyList(verb),
            });
        }
        let tail = trim(after_list);
        let extent = match (tail, is_file_wide) {
            ("", true) => LintExtent::File,
            ("", false) => LintExtent::Statement,
            ("start", false) => LintExtent::RegionStart,
            ("end", false) => LintExtent::RegionEnd,
            ("start" | "end", true) => {
                return Err(error(tail, DirectiveErrorKind::RegionOnFileWide));
            }
            _ => return Err(error(tail, DirectiveErrorKind::UnknownExtent(verb))),
        };
        Ok(Some(Self { verb, extent, list: span }))
    }

    /// The span of each rule name in the list, `*` included.
    pub fn rules(self, source: &str) -> impl Iterator<Item = Span> {
        let list = self.list.text(source);
        names(list).map(move |name| span_in(list, self.list.start, name))
    }
}

enum Command<'text> {
    FormatOff,
    FormatOn,
    FormatIgnore,
    /// A lint verb, its word, and what follows it.
    Lint {
        verb: LintVerb,
        word: &'text str,
        rest: &'text str,
    },
}

/// Why a `luck:` comment is no directive, with the word at fault.
enum Invalid<'text> {
    /// Nothing follows `luck:`.
    Empty,
    /// `#luck:` before a word other than a lint verb.
    FileWide(&'text str),
    Unknown(&'text str),
    /// `format` before a word other than `off`, `on`, or `ignore`, or before
    /// none, when it's `format` itself.
    FormatSetting(&'text str),
}

impl<'text> Invalid<'text> {
    const fn word(&self) -> Option<&'text str> {
        match *self {
            Self::Empty => None,
            Self::FileWide(word) | Self::Unknown(word) | Self::FormatSetting(word) => Some(word),
        }
    }

    const fn kind(&self) -> DirectiveErrorKind {
        match *self {
            Self::Empty => DirectiveErrorKind::Empty,
            Self::FileWide(_) => DirectiveErrorKind::NotLintVerb,
            Self::Unknown(_) => DirectiveErrorKind::UnknownDirective,
            Self::FormatSetting(_) => DirectiveErrorKind::UnknownFormatSetting,
        }
    }
}

/// What follows the `luck:` or `#luck:` that opens a directive's text, past
/// its delimiters and whitespace, and whether it's `#luck:`.
fn command(text: &str, kind: CommentKind) -> Option<(&str, bool)> {
    let body = match kind {
        CommentKind::Line => text.get(2..)?.trim_start_matches('-'),
        CommentKind::SingleLineBlock | CommentKind::MultiLineBlock => block_body(text),
        CommentKind::Shebang => return None,
    };
    let after_whitespace = trim_start(body);
    match after_whitespace.strip_prefix("#luck:") {
        Some(command) => Some((command, true)),
        None => Some((after_whitespace.strip_prefix("luck:")?, false)),
    }
}

/// A block comment's text between its brackets, or to its end when the
/// source cut it off before its closer.
fn block_body(text: &str) -> &str {
    let after_open = text.get(3..).unwrap_or_default();
    let level = after_open.bytes().take_while(|&b| b == b'=').count();
    let body = after_open.get(level.saturating_add(1)..).unwrap_or_default();
    body.strip_suffix(']')
        .and_then(|before| before.strip_suffix(after_open.get(..level)?))
        .and_then(|before| before.strip_suffix(']'))
        .unwrap_or(body)
}

/// # Errors
///
/// Returns why `command` is no directive.
fn read_command(command: &str, is_file_wide: bool) -> Result<Command<'_>, Invalid<'_>> {
    let (word, rest) = split_at(trim_start(command), |b| b == b'(' || is_lua_whitespace(b));
    let verb = match word {
        "allow" => LintVerb::Allow,
        "deny" => LintVerb::Deny,
        "warn" => LintVerb::Warn,
        "" if rest.is_empty() => return Err(Invalid::Empty),
        _ => {
            let (whole_word, after_word) = split_at(trim_start(command), is_lua_whitespace);
            return if is_file_wide {
                Err(Invalid::FileWide(whole_word))
            } else {
                read_format_command(whole_word, after_word)
            };
        }
    };
    Ok(Command::Lint { verb, word, rest })
}

/// # Errors
///
/// Returns why `word` and `rest` are no formatter directive.
fn read_format_command<'text>(
    word: &'text str,
    rest: &'text str,
) -> Result<Command<'text>, Invalid<'text>> {
    match word {
        "ignore" => Ok(Command::FormatIgnore),
        "format" => {
            let (setting, _) = split_at(trim_start(rest), is_lua_whitespace);
            match setting {
                "off" => Ok(Command::FormatOff),
                "on" => Ok(Command::FormatOn),
                "ignore" => Ok(Command::FormatIgnore),
                "" => Err(Invalid::FormatSetting(word)),
                _ => Err(Invalid::FormatSetting(setting)),
            }
        }
        _ => Err(Invalid::Unknown(word)),
    }
}

/// Each name in a rule list, trimmed, skipping empty ones like the one in
/// `allow(a,)`.
fn names(list: &str) -> impl Iterator<Item = &str> {
    list.split(',').map(trim).filter(|name| !name.is_empty())
}

/// `text` split before its first byte `ends` accepts. Every byte it accepts is
/// ASCII, so the split falls between characters.
fn split_at(text: &str, ends: impl Fn(u8) -> bool) -> (&str, &str) {
    let length = text.bytes().take_while(|&b| !ends(b)).count();
    text.split_at_checked(length).unwrap_or((text, ""))
}

fn trim_start(text: &str) -> &str {
    split_at(text, |b| !is_lua_whitespace(b)).1
}

fn trim(text: &str) -> &str {
    let start = trim_start(text);
    let length = start
        .len()
        .saturating_sub(start.bytes().rev().take_while(|&b| is_lua_whitespace(b)).count());
    start.get(..length).unwrap_or_default()
}

/// The span of `part`, a slice of `text`, which starts at `start`.
fn span_in(text: &str, start: u32, part: &str) -> Span {
    let index = part.as_ptr().addr().saturating_sub(text.as_ptr().addr());
    Span::from_indices(index, index.saturating_add(part.len())).shifted(start)
}
