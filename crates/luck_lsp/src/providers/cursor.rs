use core::slice;

use luck_ast::{
    Block, Call, ChainHead, Expression, FunctionArgs, Literal, Name, Statement, Suffix,
    SuffixedExpression, Var, Visitor,
};
use luck_semantic::{ScopeTree, SymbolId, SymbolKind};
use luck_token::{CommentKind, Span, Token, TokenKind, offset_to_index};

use crate::document::DocumentState;

/// Whether `offset` falls inside `span`, either end included, so a cursor
/// just past a name still counts as on it.
pub(crate) const fn touches(span: Span, offset: u32) -> bool {
    span.start <= offset && offset <= span.end
}

/// `name`'s text, or `None` for a name the parser found missing.
pub(crate) const fn identifier<'ast>(name: &Name<'ast>) -> Option<&'ast str> {
    if name.text.is_empty() { None } else { Some(name.text) }
}

/// The statements of `block` that touch `offset`, which are at most the two
/// that meet there.
///
/// A statement holds every node inside it, and a block's statements are in
/// source order, so a walk toward `offset` reads only these.
pub(crate) fn statements_touching<'ast>(
    block: &'ast Block<'ast>,
    offset: u32,
) -> impl Iterator<Item = &'ast Statement<'ast>> {
    let first = block.stmts.partition_point(|stmt| stmt.span().end < offset);
    let stmts: &'ast [Statement<'ast>] = &block.stmts;
    stmts
        .get(first..)
        .unwrap_or_default()
        .iter()
        .take_while(move |stmt| stmt.span().start <= offset)
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
    let parse = document.parse();
    let comments = &parse.comments;
    let at_offset = comments.get(comments.partition_point(|comment| comment.span.end < offset));
    let is_in_comment = at_offset.is_some_and(|comment| match comment.kind {
        CommentKind::Line | CommentKind::Shebang => {
            comment.span.start < offset && offset <= comment.span.end
        }
        CommentKind::SingleLineBlock | CommentKind::MultiLineBlock => {
            let is_unclosed = is_unclosed_block_comment(comment.span.text(parse.source));
            is_inside(comment.span, offset, is_unclosed)
        }
    });
    if is_in_comment {
        return TextKind::Comment;
    }
    let tokens = document.tokens();
    let enclosing = tokens.get(tokens.partition_point(|token| token.span.end < offset));
    let is_in_string = enclosing.is_some_and(|token| {
        matches!(
            token.kind,
            TokenKind::StringLiteral
                | TokenKind::InterpolatedStringBegin
                | TokenKind::InterpolatedStringMiddle
                | TokenKind::InterpolatedStringEnd
                | TokenKind::InterpolatedStringSimple
        ) && is_inside(token.span, offset, token.is_unterminated)
    });
    if is_in_string { TextKind::String } else { TextKind::Code }
}

/// Whether `offset` is inside the string token or block comment at `span`:
/// past its first byte, and before its end unless a line break or the end of
/// the text cut it off there.
const fn is_inside(span: Span, offset: u32, is_cut_off: bool) -> bool {
    span.start < offset && (offset < span.end || (offset == span.end && is_cut_off))
}

/// Whether a block comment lacks the closing bracket of its opening one's
/// level, which leaves it open to the end of the source.
fn is_unclosed_block_comment(comment: &str) -> bool {
    let after_opening = comment.get("--[".len()..).unwrap_or_default();
    let level = after_opening.bytes().take_while(|&b| b == b'=').count();
    let body = after_opening.get(level.saturating_add(1)..).unwrap_or_default();
    let closer = body
        .len()
        .checked_sub(level.saturating_add(2))
        .and_then(|start| body.as_bytes().get(start..))
        .unwrap_or_default();
    let Some((b']', [equals @ .., b']'])) = closer.split_first() else {
        return true;
    };
    !equals.iter().all(|&b| b == b'=')
}

/// Whether the string token starting where `literal` does runs to the end of
/// its line or of the source without its closing delimiter.
pub(crate) fn is_unterminated(document: &DocumentState, literal: &Literal<'_>) -> bool {
    let tokens = document.tokens();
    tokens
        .binary_search_by_key(&literal.span.start, |token| token.span.start)
        .ok()
        .and_then(|i| tokens.get(i))
        .is_some_and(|token| token.is_unterminated)
}

/// The plain-name path `head` ends with (`a.b.c`), root first, and the
/// root's span: a name, or the fields it ends in and the name before them.
///
/// A chain broken by an index expression, a call, or a non-name prefix
/// yields only the trailing fields that are plain names. `None` when `head`
/// ends in nothing named by an identifier.
fn name_path(head: ChainHead<'_>) -> Option<(Vec<&str>, Span)> {
    let Some((mut before, Suffix::Field(last))) = head.split_last() else {
        let name = head.as_name()?;
        return Some((vec![identifier(name)?], name.span));
    };
    let mut path = vec![identifier(last)?];
    let mut root_span = last.span;
    while let Some((earlier, Suffix::Field(inner))) = before.split_last() {
        let Some(segment) = identifier(inner) else {
            break;
        };
        path.push(segment);
        root_span = inner.span;
        before = earlier;
    }
    if let Some(root) = before.as_name()
        && let Some(segment) = identifier(root)
    {
        path.push(segment);
        root_span = root.span;
    }
    path.reverse();
    Some((path, root_span))
}

/// The `Name(.Name)*` chain `tokens` ends with, root first, and the root's
/// span. `None` when anything else, such as a call, breaks the chain.
pub(crate) fn dotted_chain_ending_tokens<'source>(
    tokens: &[Token],
    source: &'source str,
) -> Option<(Vec<&'source str>, Span)> {
    let mut segments: Vec<&str> = Vec::new();
    let mut remaining_tokens = tokens;
    loop {
        let (base, before_base) = remaining_tokens.split_last()?;
        if base.kind != TokenKind::Identifier {
            return None;
        }
        segments.push(base.text(source));
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
    Identifier(&'ast Name<'ast>),
    /// A dotted path like `string.format`, of name-only segments.
    DottedPath {
        path: Vec<&'ast str>,
        root_span: Span,
        full_span: Span,
    },
    /// Path is empty for indirect callees.
    Call {
        path: Vec<&'ast str>,
        /// Span of the root name, when the callee is a plain dotted chain.
        /// Lets consumers resolve shaped/shadowed roots.
        root_span: Option<Span>,
        full_span: Span,
    },
}

impl<'ast> CursorNode<'ast> {
    /// The path segments; a single entry for a plain identifier.
    pub(crate) fn path(&self) -> &[&'ast str] {
        match self {
            Self::Identifier(name) => slice::from_ref(&name.text),
            Self::DottedPath { path, .. } | Self::Call { path, .. } => path,
        }
    }

    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Identifier(name) => name.span,
            Self::DottedPath { full_span, .. } | Self::Call { full_span, .. } => *full_span,
        }
    }
}

/// Find the smallest name, field access, or call expression whose span
/// contains `offset`.
pub(crate) fn find_node_at<'ast>(
    block: &'ast Block<'ast>,
    offset: u32,
) -> Option<CursorNode<'ast>> {
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

    fn record_name(&mut self, name: &'ast Name<'ast>) {
        if identifier(name).is_some() {
            self.try_set(CursorNode::Identifier(name));
        }
    }

    /// Of the field accesses and calls in `chain`, only the shortest one
    /// that reaches the cursor can be the smallest node there.
    fn record_chain(&mut self, chain: &'ast SuffixedExpression<'ast>) {
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
                Suffix::Call(_) | Suffix::MethodCall(_) => {
                    if let Some(call) = Call::new(chain.head(index), suffix) {
                        self.record_call(call);
                    }
                }
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
        if let Some(method) = call.method {
            if let Some(name) = identifier(method)
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
    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        for stmt in statements_touching(block, self.offset) {
            self.visit_stmt(stmt);
        }
        if let Some(last) = &block.last_stmt
            && touches(last.span(), self.offset)
        {
            self.visit_last_stmt(last);
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if !touches(expr.span(), self.offset) {
            return;
        }
        if let Expression::Name(name) = expr {
            self.record_name(name);
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &'ast Var<'ast>) {
        if let Var::Name(name) = target {
            self.record_name(name);
        }
        self.walk_assignment_target(target);
    }

    fn visit_suffixed(&mut self, chain: &'ast SuffixedExpression<'ast>) {
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
    finder.visit_block(&document.parse().block);
    finder.result
}

struct CallSiteFinder<'document> {
    offset: u32,
    source: &'document str,
    tokens: &'document [Token],
    result: Option<CallSite<'document>>,
}

impl<'document> Visitor<'document> for CallSiteFinder<'document> {
    fn visit_block(&mut self, block: &'document Block<'document>) {
        for stmt in statements_touching(block, self.offset) {
            self.visit_stmt(stmt);
        }
        if let Some(last) = &block.last_stmt
            && touches(last.span(), self.offset)
        {
            self.visit_last_stmt(last);
        }
    }

    fn visit_expr(&mut self, expr: &'document Expression<'document>) {
        if touches(expr.span(), self.offset) {
            self.walk_expr(expr);
        }
    }

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
    fn is_followed_by_comma_before_cursor(&self, arg: &Expression<'_>) -> bool {
        let next_token = self.tokens.partition_point(|token| token.span.start < arg.span().end);
        self.tokens
            .get(next_token)
            .is_some_and(|token| token.kind == TokenKind::Comma && token.span.end <= self.offset)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StringArgSite<'ast> {
    pub call: Call<'ast>,
    pub arg_index: usize,
    pub literal: &'ast Literal<'ast>,
}

/// Find the innermost call whose argument list contains a string
/// literal enclosing `offset` (cursor strictly inside the quotes).
pub(crate) fn find_string_arg_at(
    document: &DocumentState,
    offset: u32,
) -> Option<StringArgSite<'_>> {
    let mut finder = StringArgFinder { document, offset, result: None };
    finder.visit_block(&document.parse().block);
    finder.result
}

struct StringArgFinder<'ast> {
    document: &'ast DocumentState,
    offset: u32,
    result: Option<StringArgSite<'ast>>,
}

impl<'ast> Visitor<'ast> for StringArgFinder<'ast> {
    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        for stmt in statements_touching(block, self.offset) {
            self.visit_stmt(stmt);
        }
        if let Some(last) = &block.last_stmt
            && touches(last.span(), self.offset)
        {
            self.visit_last_stmt(last);
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if touches(expr.span(), self.offset) {
            self.walk_expr(expr);
        }
    }

    fn visit_call(&mut self, call: Call<'ast>) {
        self.try_record(call);
        self.walk_call(call);
    }
}

impl<'ast> StringArgFinder<'ast> {
    /// Inside the quotes, so a cursor next to the literal doesn't trigger
    /// value completion.
    fn contains(&self, literal: &Literal<'_>) -> bool {
        let span = literal.span;
        span.start < self.offset
            && (self.offset < span.end
                || (self.offset == span.end && is_unterminated(self.document, literal)))
    }

    fn try_record(&mut self, call: Call<'ast>) {
        let site = match call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                args.iter().enumerate().find_map(|(index, arg)| {
                    if let Expression::StringLiteral(literal) = arg
                        && self.contains(literal)
                    {
                        Some((index, &**literal))
                    } else {
                        None
                    }
                })
            }
            FunctionArgs::StringLiteral(literal) if self.contains(literal) => Some((0, literal)),
            FunctionArgs::StringLiteral(_)
            | FunctionArgs::TableConstructor(_)
            | FunctionArgs::Error(_) => None,
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
pub(crate) fn local_at(tree: &ScopeTree<'_>, offset: u32) -> Option<(SymbolId, Span)> {
    if let Some(reference) = tree.reference_at(offset).map(|id| tree.reference(id)) {
        return Some((reference.resolved?, reference.span));
    }
    tree.symbols()
        .iter()
        .find(|symbol| {
            symbol.kind != SymbolKind::ImplicitSelf && touches(symbol.definition_span, offset)
        })
        .map(|symbol| (symbol.id, symbol.definition_span))
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;

    use super::*;
    use crate::fixtures::document;

    /// The kind of text at each `|` in `marked`, which the text holds without
    /// them.
    fn kinds_at_marks(marked: &str, target: LuaTarget) -> Vec<TextKind> {
        let text = marked.replace('|', "");
        let document = document(&text, target);
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
            ("x = 1 |--[==[ a ]]|", LuaTarget::Lua54, vec![Code, Comment]),
            (
                "\
--[[ a
x = \"b|\"
|",
                LuaTarget::Lua54,
                vec![Comment, Comment],
            ),
            ("--[[ a [[ b ]]|", LuaTarget::Lua51, vec![Code]),
            ("--[[]]|", LuaTarget::Lua54, vec![Code]),
            ("--[[]|", LuaTarget::Lua54, vec![Comment]),
            ("--[==[ ]=] ]==]|", LuaTarget::Lua54, vec![Code]),
        ] {
            assert_eq!(kinds_at_marks(marked, target), expected, "{marked:?}");
        }
    }

    #[test]
    fn a_walk_toward_the_cursor_reads_only_the_statements_that_touch_it() {
        let document = document("f()g() h()", LuaTarget::Lua54);
        let block = &document.parse().block;
        let spans_at = |offset| {
            statements_touching(block, offset).map(|stmt| (stmt.span().start, stmt.span().end))
        };
        assert_eq!(spans_at(3).collect::<Vec<_>>(), [(0, 3), (3, 6)]);
        assert_eq!(spans_at(7).collect::<Vec<_>>(), [(7, 10)]);
        assert_eq!(spans_at(6).collect::<Vec<_>>(), [(3, 6)]);
    }
}
