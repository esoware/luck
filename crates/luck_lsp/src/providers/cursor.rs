use core::slice;

use luck_ast::{
    Block, Call, ChainHead, Expression, FunctionArgs, Literal, Suffix, SuffixedExpression, Var,
    Visitor,
};
use luck_semantic::{ScopeTree, SymbolId, SymbolKind};
use luck_token::{CommentKind, Span, Token, TokenKind, offset_to_index};

use crate::document::DocumentState;

/// Whether `offset` falls inside `span`, either end included, so a cursor
/// just past a name still counts as on it.
pub(crate) const fn touches(span: Span, offset: u32) -> bool {
    span.start <= offset && offset <= span.end
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextKind {
    Code,
    Comment,
    String,
}

/// Text that a closing delimiter ends counts up to it, and text a line break
/// or the end of the file cut off counts through its end, where typing goes
/// on inside it.
pub(crate) fn text_kind_at(document: &DocumentState, offset: u32) -> TextKind {
    let comments = &document.parsed.comments;
    let at_offset = comments.get(comments.partition_point(|comment| comment.span.end < offset));
    let is_in_comment = at_offset.is_some_and(|comment| {
        let has_room = match comment.kind {
            CommentKind::Line | CommentKind::Shebang => offset <= comment.span.end,
            CommentKind::SingleLineBlock | CommentKind::MultiLineBlock => offset < comment.span.end,
        };
        comment.span.start < offset && has_room
    });
    // An unterminated block comment is a lex error over its text rather than
    // a comment, and no other error's text opens with `--`.
    let is_in_unterminated_comment = document.parsed.errors.iter().any(|e| {
        e.span.start < offset
            && offset <= e.span.end
            && e.span.text(document.text()).starts_with("--")
    });
    if is_in_comment || is_in_unterminated_comment {
        return TextKind::Comment;
    }
    let tokens = document.tokens();
    let enclosing = tokens.get(tokens.partition_point(|token| token.span.end < offset));
    let is_in_string = enclosing.is_some_and(|token| {
        matches!(
            token.kind,
            TokenKind::StringLiteral(_)
                | TokenKind::InterpolatedStringBegin(_)
                | TokenKind::InterpolatedStringMiddle(_)
                | TokenKind::InterpolatedStringEnd(_)
        ) && is_inside(document, token.span, offset)
    });
    if is_in_string { TextKind::String } else { TextKind::Code }
}

/// Whether `offset` is inside the string token at `span`: past its first
/// byte, and before its end unless a line break or the end of the text cut it
/// off there.
fn is_inside(document: &DocumentState, span: Span, offset: u32) -> bool {
    span.start < offset
        && (offset < span.end
            || (offset == span.end && document.unterminated_strings().contains(&span)))
}

/// The plain-name path `head` ends with (`a.b.c`), root first, and the
/// root's span: a name, or the fields it ends in and the name before them.
///
/// A chain broken by an index expression, a call, or a non-name prefix
/// yields only the trailing fields that are plain names. `None` when `head`
/// ends in nothing named by an identifier.
fn name_path(head: ChainHead<'_>) -> Option<(Vec<&str>, Span)> {
    let Some((mut before, Suffix::Field(name))) = head.split_last() else {
        let token = head.as_name()?;
        return Some((vec![token.identifier()?], token.span));
    };
    let mut path = vec![name.identifier()?];
    let mut root_span = name.span;
    while let Some((earlier, Suffix::Field(inner))) = before.split_last() {
        let Some(segment) = inner.identifier() else {
            break;
        };
        path.push(segment);
        root_span = inner.span;
        before = earlier;
    }
    if let Some(token) = before.as_name()
        && let Some(segment) = token.identifier()
    {
        path.push(segment);
        root_span = token.span;
    }
    path.reverse();
    Some((path, root_span))
}

/// The `Name(.Name)*` chain `tokens` ends with, root first, and the root's
/// span. `None` when anything else, such as a call, breaks the chain.
pub(crate) fn dotted_chain_ending_tokens(tokens: &[Token]) -> Option<(Vec<&str>, Span)> {
    let mut segments: Vec<&str> = Vec::new();
    let mut remaining_tokens = tokens;
    loop {
        let (base, before_base) = remaining_tokens.split_last()?;
        let TokenKind::Identifier(base_name) = &base.kind else {
            return None;
        };
        segments.push(base_name.as_str());
        if let Some((Token { kind: TokenKind::Dot, .. }, before_dot)) = before_base.split_last() {
            remaining_tokens = before_dot;
        } else {
            segments.reverse();
            return Some((segments, base.span));
        }
    }
}

#[derive(Debug)]
pub(crate) enum CursorNode<'ast> {
    Identifier {
        name: &'ast str,
        span: Span,
    },
    /// A dotted path like `string.format`, of name-only segments.
    DottedPath {
        path: Vec<&'ast str>,
        root_span: Span,
        full_span: Span,
    },
    /// Path is empty for indirect callees.
    Call {
        path: Vec<&'ast str>,
        /// Span of the root `Name` token, when the callee is a plain
        /// dotted chain. Lets consumers resolve shaped/shadowed roots.
        root_span: Option<Span>,
        full_span: Span,
    },
}

impl<'ast> CursorNode<'ast> {
    /// The path segments; a single entry for a plain identifier.
    pub(crate) fn path(&self) -> &[&'ast str] {
        match self {
            Self::Identifier { name, .. } => slice::from_ref(name),
            Self::DottedPath { path, .. } | Self::Call { path, .. } => path,
        }
    }

    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Identifier { span, .. } => *span,
            Self::DottedPath { full_span, .. } | Self::Call { full_span, .. } => *full_span,
        }
    }
}

/// Find the smallest name, field access, or call expression whose span
/// contains `offset`.
pub(crate) fn find_node_at(block: &Block, offset: u32) -> Option<CursorNode<'_>> {
    let mut finder = NodeFinder { offset, result: None };
    finder.visit_block(block);
    finder.result
}

struct NodeFinder<'ast> {
    offset: u32,
    result: Option<CursorNode<'ast>>,
}

impl<'ast> NodeFinder<'ast> {
    fn try_set(&mut self, node: CursorNode<'ast>) {
        let span = node.span();
        if !touches(span, self.offset) {
            return;
        }
        if self.result.as_ref().is_none_or(|existing| span.len() <= existing.span().len()) {
            self.result = Some(node);
        }
    }

    fn record_name(&mut self, token: &'ast Token) {
        if let Some(name) = token.identifier() {
            self.try_set(CursorNode::Identifier { name, span: token.span });
        }
    }

    /// Of the field accesses and calls in `chain`, only the shortest one
    /// that reaches the cursor can be the smallest node there.
    fn record_chain(&mut self, chain: &'ast SuffixedExpression) {
        for (index, suffix) in chain.suffixes.iter().enumerate() {
            let head = chain.head(index.saturating_add(1));
            let full_span = head.span();
            if !touches(full_span, self.offset) {
                continue;
            }
            match suffix {
                Suffix::Field(_) => {
                    if let Some((path, root_span)) = name_path(head) {
                        self.try_set(CursorNode::DottedPath { path, root_span, full_span });
                    }
                }
                Suffix::Call(call) => self.record_call(Call::new(chain.head(index), call)),
                Suffix::Index { .. } | Suffix::TypeInstantiation { .. } => continue,
            }
            return;
        }
    }

    fn record_call(&mut self, call: Call<'ast>) {
        let (mut path, mut root_span) = name_path(call.callee)
            .map_or_else(|| (Vec::new(), None), |(path, span)| (path, Some(span)));
        // The method name is a path segment too: `game:GetService(...)`
        // resolves as `["game", "GetService"]` through shaped lookup.
        if let Some(method_token) = call.method {
            if let Some(name) = method_token.identifier()
                && !path.is_empty()
            {
                path.push(name);
            } else {
                path.clear();
                root_span = None;
            }
        }
        self.try_set(CursorNode::Call { path, root_span, full_span: call.span() });
    }
}

impl<'ast> Visitor<'ast> for NodeFinder<'ast> {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Name(token) = expr {
            self.record_name(token);
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &'ast Var) {
        if let Var::Name(token) = target {
            self.record_name(token);
        }
        self.walk_assignment_target(target);
    }

    fn visit_suffixed(&mut self, chain: &'ast SuffixedExpression) {
        self.record_chain(chain);
        self.walk_suffixed(chain);
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CallSite<'ast> {
    /// The call, so consumers can resolve the callee semantically (shaped
    /// locals, shadow checks).
    pub call: Call<'ast>,
    pub active_param: usize,
}

pub(crate) fn find_call_site_at(document: &DocumentState, offset: u32) -> Option<CallSite<'_>> {
    let mut finder =
        CallSiteFinder { offset, source: document.text(), tokens: document.tokens(), result: None };
    finder.visit_block(&document.parsed.block);
    finder.result
}

struct CallSiteFinder<'document> {
    offset: u32,
    source: &'document str,
    tokens: &'document [Token],
    result: Option<CallSite<'document>>,
}

impl<'document> Visitor<'document> for CallSiteFinder<'document> {
    fn visit_call(&mut self, call: Call<'document>) {
        self.try_record(call);
        self.walk_call(call);
    }
}

impl<'document> CallSiteFinder<'document> {
    fn try_record(&mut self, call: Call<'document>) {
        // `f"..."` and `f{...}` have no parens for the cursor to be inside.
        let FunctionArgs::Parenthesized { span: parens, args } = call.args else {
            return;
        };
        // An unclosed list ends where the parser stopped, so the end still
        // counts as inside it. Past a real `)` the cursor has left the call.
        let is_closed = self.source.as_bytes().get(offset_to_index(parens.end.saturating_sub(1)))
            == Some(&b')');
        if self.offset <= parens.start
            || self.offset > parens.end
            || (is_closed && self.offset == parens.end)
        {
            return;
        }
        let active_param =
            args.iter().filter(|arg| self.is_followed_by_comma_before_cursor(arg)).count();
        self.result = Some(CallSite { call, active_param });
    }

    /// Whether the token after `arg` is a comma the cursor is past. Reading
    /// the next token, rather than counting commas, can't mistake one inside
    /// a nested argument for a separator of this list.
    fn is_followed_by_comma_before_cursor(&self, arg: &Expression) -> bool {
        let next_token = self.tokens.partition_point(|token| token.span.start < arg.span().end);
        self.tokens.get(next_token).is_some_and(|token| {
            matches!(token.kind, TokenKind::Comma) && token.span.end <= self.offset
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StringArgSite<'ast> {
    pub call: Call<'ast>,
    pub arg_index: usize,
    pub literal: &'ast Literal,
}

/// Find the innermost call whose argument list contains a string
/// literal enclosing `offset` (cursor strictly inside the quotes).
pub(crate) fn find_string_arg_at(
    document: &DocumentState,
    offset: u32,
) -> Option<StringArgSite<'_>> {
    let mut finder = StringArgFinder { document, offset, result: None };
    finder.visit_block(&document.parsed.block);
    finder.result
}

struct StringArgFinder<'ast> {
    document: &'ast DocumentState,
    offset: u32,
    result: Option<StringArgSite<'ast>>,
}

impl<'ast> Visitor<'ast> for StringArgFinder<'ast> {
    fn visit_call(&mut self, call: Call<'ast>) {
        self.try_record(call);
        self.walk_call(call);
    }
}

impl<'ast> StringArgFinder<'ast> {
    /// Inside the quotes, so a cursor next to the literal doesn't trigger
    /// value completion.
    fn contains(&self, span: Span) -> bool {
        is_inside(self.document, span, self.offset)
    }

    fn try_record(&mut self, call: Call<'ast>) {
        let site = match call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                args.iter().enumerate().find_map(|(index, arg)| {
                    if let Expression::StringLiteral(literal) = arg
                        && self.contains(literal.span)
                    {
                        Some((index, literal))
                    } else {
                        None
                    }
                })
            }
            FunctionArgs::StringLiteral(literal) if self.contains(literal.span) => {
                Some((0, literal))
            }
            FunctionArgs::StringLiteral(_) | FunctionArgs::TableConstructor(_) => None,
        };
        let Some((arg_index, literal)) = site else {
            return;
        };
        // Nested calls visit outer-first; the innermost match wins.
        self.result = Some(StringArgSite { call, arg_index, literal });
    }
}

/// The local symbol at a byte offset and the name span the offset sits in:
/// a reference to the local, or its declaration.
///
/// `None` for globals, keywords, and non-names. Matching is by span through
/// the scope tree, so shadowed names map to the right declaration. A name
/// parse recovery made up is nothing to find, and neither is the implicit
/// `self`'s declaration, which no token spells.
pub(crate) fn local_at(tree: &ScopeTree, offset: u32) -> Option<(SymbolId, Span)> {
    if let Some(reference) = tree
        .references
        .iter()
        .find(|reference| !reference.name.is_empty() && touches(reference.span, offset))
    {
        return Some((reference.resolved?, reference.span));
    }
    tree.symbols
        .iter()
        .find(|symbol| {
            symbol.kind != SymbolKind::ImplicitSelf
                && !symbol.name.is_empty()
                && touches(symbol.definition_span, offset)
        })
        .map(|symbol| (symbol.id, symbol.definition_span))
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;

    use super::*;

    /// The kind of text at each `|` in `marked`, which the text holds without
    /// them.
    fn kinds_at_marks(marked: &str, target: LuaTarget) -> Vec<TextKind> {
        let text = marked.replace('|', "");
        let document = DocumentState::new(text, 1, target, FxHashSet::default());
        let mut offsets = Vec::new();
        for (index, (position, _)) in marked.match_indices('|').enumerate() {
            offsets.push(u32::try_from(position.saturating_sub(index)).expect("a small offset"));
        }
        offsets.into_iter().map(|offset| text_kind_at(&document, offset)).collect()
    }

    #[test]
    fn a_string_cut_off_by_its_line_holds_its_end() {
        use TextKind::{Code, Comment, String};
        for (marked, target, expected) in [
            ("x = `{|y}tail|", LuaTarget::Luau, vec![Code, String]),
            ("x = `{y}ta|il`|", LuaTarget::Luau, vec![String, Code]),
            ("x = `|tail|", LuaTarget::Luau, vec![String, String]),
            ("x = `|", LuaTarget::Luau, vec![String]),
            ("x = `a|`|", LuaTarget::Luau, vec![String, Code]),
            ("x = \"a|bc|", LuaTarget::Lua54, vec![String, String]),
            ("x = \"abc\"|", LuaTarget::Lua54, vec![Code]),
            (
                r#"x = "a\"|
y = 1|"#,
                LuaTarget::Lua54,
                vec![String, Code],
            ),
            ("x = [[a|", LuaTarget::Lua54, vec![String]),
            ("x = [[a]]|", LuaTarget::Lua54, vec![Code]),
            ("--[[ a|", LuaTarget::Lua54, vec![Comment]),
            ("--[[ a ]]|", LuaTarget::Lua54, vec![Code]),
        ] {
            assert_eq!(kinds_at_marks(marked, target), expected, "{marked:?}");
        }
    }
}
