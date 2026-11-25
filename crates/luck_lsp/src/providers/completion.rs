//! Known limitation: a literal receiver mid-typing (`("x"):` with nothing
//! after the colon) gets no member completion. The dangling colon doesn't
//! parse, so there's no AST to resolve the receiver from.

use std::ptr;

use ls_types::{
    CompletionItem, CompletionItemKind, CompletionItemTag, CompletionResponse, CompletionTextEdit,
    Documentation, InsertTextFormat, MarkupContent, MarkupKind, Position, TextEdit,
};
use luck_ast::{AstKind, Block, Call, Expression, FunctionArgs, StringLiteral};
use luck_stdlib::{StdlibEntry, StdlibFunction};
use luck_token::{Span, Token, TokenKind, index_to_offset};
use rustc_hash::FxHashSet;

use crate::document::{DocumentState, span_text};
use crate::providers::cursor::{TextKind, is_inside, is_unterminated, name_at, text_kind_at};
use crate::providers::stdlib_render::param_label;

pub(crate) fn completion(document: &DocumentState, position: Position) -> CompletionResponse {
    let offset = document.offset(position);
    if let Some(items) = constant_value_completions(document, offset) {
        return CompletionResponse::Array(items);
    }
    if text_kind_at(document, offset) != TextKind::Code {
        return CompletionResponse::Array(Vec::new());
    }
    let items = match member_access_before(document, offset) {
        Some(access) => member_completions(document, &access, offset),
        None => bare_name_completions(document, offset),
    };
    CompletionResponse::Array(items)
}

fn bare_name_completions(document: &DocumentState, offset: u32) -> Vec<CompletionItem> {
    let semantic = document.semantic();
    let mut items: Vec<CompletionItem> =
        semantic.stdlib().globals.iter().map(|(name, entry)| item_for_entry(name, entry)).collect();
    // Every local declared before the cursor in a scope open there, each name
    // once. The implicit `self` isn't spelled anywhere, and a name recovery
    // made up is empty.
    let tree = semantic.scoping();
    let tokens = document.tokens();
    // Code typed at the cursor is in a block from the token that opens it to
    // the one that closes it, so a blank line that ends a block, or a body
    // still missing its `end`, is in it.
    let is_in_block = |block: &Block<'_>| {
        let first = tokens.partition_point(|token| token.span.start < block.span.start);
        let opener = tokens[..first].last();
        let closer = if block.stmts.is_empty() {
            first
        } else {
            tokens.partition_point(|token| token.span.start < block.span.end)
        };
        opener.is_none_or(|token| token.span.end <= offset)
            && tokens.get(closer).is_none_or(|token| offset <= token.span.start)
    };
    let nodes = document.nodes();
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "a scope's node is one of the few kinds that open one"
    )]
    let is_open: Vec<bool> = tree
        .scopes()
        .iter()
        .map(|scope| {
            scope.parent.is_none()
                || match nodes.kind(scope.node) {
                    AstKind::Block(block) => is_in_block(block),
                    AstKind::FunctionBody(body) => is_in_block(&body.block),
                    AstKind::NumericFor(numeric_for) => is_in_block(&numeric_for.block),
                    AstKind::GenericFor(generic_for) => is_in_block(&generic_for.block),
                    // The `until` condition is in the body's scope.
                    AstKind::RepeatLoop(repeat_loop) => {
                        is_in_block(&repeat_loop.block)
                            || (repeat_loop.block.span.end <= offset
                                && offset <= repeat_loop.span.end)
                    }
                    _ => false,
                }
        })
        .collect();
    let mut offered: FxHashSet<&str> = FxHashSet::default();
    for symbol in tree.symbols() {
        let name = tree.name(symbol.name);
        if !symbol.kind.is_implicit()
            && !name.is_empty()
            && symbol.definition_span.end <= offset
            && is_open[symbol.scope.index()]
            && offered.insert(name)
        {
            items.push(CompletionItem {
                label: name.to_owned(),
                kind: Some(CompletionItemKind::VARIABLE),
                ..Default::default()
            });
        }
    }
    items.extend(KEYWORD_COMPLETIONS.iter().map(|keyword| CompletionItem {
        label: (*keyword).to_owned(),
        kind: Some(CompletionItemKind::KEYWORD),
        ..Default::default()
    }));
    items
}

/// After `string.`, `Enum.Material.`, or `Enum.Material:`, the receiver's
/// members: non-methods after a dot, methods after a colon.
fn member_completions(
    document: &DocumentState,
    access: &MemberAccess<'_>,
    offset: u32,
) -> Vec<CompletionItem> {
    let Some((segments, root_span)) = &access.receiver else {
        return Vec::new();
    };
    let Some(&root) = segments.first() else {
        return Vec::new();
    };
    let semantic = document.semantic();
    let tree = semantic.scoping();
    // A reference at the root is exact. Text mid-typing often has none, so
    // that case falls back to the nearest declaration before the cursor.
    let reference_id = name_at(document, root_span.start)
        .filter(|(_, name)| name.span == *root_span)
        .and_then(|(_, name)| tree.reference_of(name));
    let (root_local, reaches_host_global) = if let Some(reference_id) = reference_id {
        let reference = tree.reference(reference_id);
        (reference.resolved, tree.is_host_global(reference))
    } else {
        let local = tree.nearest_local(root, offset).map(|symbol| symbol.id);
        (local, local.is_none())
    };

    let stdlib = semantic.stdlib();
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut push = |name: &str, member: &StdlibEntry<'_>| {
        let is_method = matches!(member, StdlibEntry::Function(function) if function.is_method);
        if is_method == access.is_colon {
            items.push(item_for_entry(name, member));
        }
    };
    let stdlib_entry = reaches_host_global.then(|| stdlib.lookup(segments)).flatten();
    if let Some(StdlibEntry::Namespace(namespace)) = stdlib_entry
        && !access.is_colon
    {
        for (name, member) in &namespace.members {
            push(name, member);
        }
    }
    // A shaped local: `local g = game; g:` offers the DataModel methods, its
    // Instance ones among them.
    let shape = match (stdlib_entry, root_local, segments.as_slice()) {
        (Some(entry), _, _) => entry.shape(),
        (None, Some(symbol_id), [_]) => semantic.symbol_shape(symbol_id),
        (None, _, _) => None,
    };
    if let Some(shape_name) = shape {
        for (name, member) in stdlib.shape_members(shape_name) {
            push(name, member);
        }
    }
    items
}

/// The literal must be a positional argument whose parameter is constant-typed
/// in every signature matching the call's arity. `None` falls through to
/// ordinary identifier completion.
fn constant_value_completions(
    document: &DocumentState,
    offset: u32,
) -> Option<Vec<CompletionItem>> {
    // The string at the cursor, which only counts inside its quotes, so a
    // cursor next to it doesn't trigger value completion, and the call it's
    // an argument of, whose chain holds it or the method call that does.
    let nodes = document.nodes();
    let id = nodes.node_at(offset)?;
    let literal = nodes.kind(id).as_string_literal()?;
    let is_unterminated = is_unterminated(document, literal);
    if !is_inside(literal.span, offset, is_unterminated) {
        return None;
    }
    let parent = nodes.parent_id(id)?;
    let chain = if matches!(nodes.kind(parent), AstKind::MethodCall(_)) {
        nodes.parent_kind(parent)?.as_suffixed_expr()?
    } else {
        nodes.kind(parent).as_suffixed_expr()?
    };
    let (call, arg_index) = chain.calls().find_map(|(_, call)| {
        let index = match call.args {
            FunctionArgs::Parenthesized { args, .. } => args.iter().position(|arg| {
                matches!(arg, Expression::StringLiteral(string) if ptr::eq(string.as_ref(), literal))
            })?,
            FunctionArgs::StringLiteral(arg) if ptr::eq(arg.as_ref(), literal) => 0,
            FunctionArgs::StringLiteral(_)
            | FunctionArgs::TableConstructor(_)
            | FunctionArgs::Error(_) => return None,
        };
        Some((call, index))
    })?;
    let callee = document.semantic().resolve_callee(call)?;
    let StdlibEntry::Function(function) = callee.entry() else {
        return None;
    };
    let allowed_sets = function.allowed_constants(arg_index, call_arg_count(call))?;
    // Replace the literal's interior, so the client filters on whatever is
    // typed inside the delimiters.
    let interior = document.range(string_interior(literal, is_unterminated));
    let mut seen_values: FxHashSet<&str> = FxHashSet::default();
    let mut items: Vec<CompletionItem> = Vec::new();
    for constant in allowed_sets.into_iter().flatten() {
        if !seen_values.insert(constant.value) {
            continue;
        }
        let is_deprecated = constant.deprecated.is_some();
        // Rank: curated common values, then the rest, deprecated last.
        let rank = if is_deprecated {
            '2'
        } else if constant.is_common {
            '0'
        } else {
            '1'
        };
        items.push(CompletionItem {
            label: constant.value.to_owned(),
            kind: Some(CompletionItemKind::ENUM_MEMBER),
            sort_text: Some(format!("{rank}{}", constant.value)),
            filter_text: Some(constant.value.to_owned()),
            deprecated: Some(is_deprecated),
            tags: is_deprecated.then(|| vec![CompletionItemTag::DEPRECATED]),
            documentation: constant.deprecated.as_ref().map(|deprecation| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("**Deprecated:** {}", deprecation.message),
                })
            }),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: interior,
                new_text: constant.value.to_owned(),
            })),
            ..Default::default()
        });
    }
    Some(items)
}

fn call_arg_count(call: Call<'_>) -> usize {
    match call.args {
        FunctionArgs::Parenthesized { args, .. } => args.len(),
        FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => 1,
        FunctionArgs::Error(_) => 0,
    }
}

/// The part of a string literal between its delimiters. A long bracket's
/// opener is `[`, any number of `=`, then `[`, and its closer mirrors it. An
/// unterminated literal has no closer.
fn string_interior(literal: &StringLiteral<'_>, is_unterminated: bool) -> Span {
    let raw = literal.raw.as_deref().unwrap_or_default();
    let delimiter_width =
        index_to_offset(raw.strip_prefix(b"[").map_or(1, |after_bracket| {
            after_bracket.iter().take_while(|&&b| b == b'=').count() + 2
        }));
    let closer_width = if is_unterminated { 0 } else { delimiter_width };
    Span::new(literal.span.start + delimiter_width, literal.span.end.saturating_sub(closer_width))
}

struct MemberAccess<'source> {
    /// The receiver's dotted path before the final separator, root first
    /// (`Enum.Material.` gives `["Enum", "Material"]`), and the root's span.
    /// `None` for a receiver that isn't a plain name chain, such as `f().`.
    receiver: Option<(Vec<&'source str>, Span)>,
    /// The final separator was `:`, so complete methods instead of members.
    is_colon: bool,
}

/// The tokens before the cursor end in a `.` or `:`, optionally followed by a
/// partial name (`a.b.` or `a.b:fo`). Working from tokens keeps a `..` concat
/// from reading as a member separator.
fn member_access_before(document: &DocumentState, offset: u32) -> Option<MemberAccess<'_>> {
    let (source, tokens) = (document.text(), document.tokens());
    let before_cursor = &tokens[..tokens.partition_point(|token| token.span.start < offset)];
    let (last, before_last) = before_cursor.split_last()?;
    // The word in progress can lex as a keyword: `string.for` is on its way
    // to `format`.
    let is_partial_word =
        offset <= last.span.end && TokenKind::is_identifier_shaped(span_text(source, last.span));
    let (separator, before_separator) =
        if is_partial_word { before_last.split_last()? } else { (last, before_last) };
    let is_colon = separator.kind == TokenKind::Colon;
    if !is_colon && separator.kind != TokenKind::Dot {
        return None;
    }
    Some(MemberAccess { receiver: dotted_chain_ending_tokens(before_separator, source), is_colon })
}

/// The `Name(.Name)*` chain `tokens` ends with, root first, and the root's
/// span. `None` when anything else, such as a call, breaks the chain.
fn dotted_chain_ending_tokens<'source>(
    tokens: &[Token],
    source: &'source str,
) -> Option<(Vec<&'source str>, Span)> {
    let mut segments: Vec<&str> = Vec::new();
    let mut remaining_tokens = tokens;
    loop {
        let (base, before_base) = remaining_tokens.split_last()?;
        if !base.kind.is_name() {
            return None;
        }
        segments.push(span_text(source, base.span));
        if let Some((Token { kind: TokenKind::Dot, .. }, before_dot)) = before_base.split_last() {
            remaining_tokens = before_dot;
        } else {
            segments.reverse();
            return Some((segments, base.span));
        }
    }
}

fn item_for_entry(name: &str, entry: &StdlibEntry<'_>) -> CompletionItem {
    let (kind, detail, is_deprecated, markdown) = match entry {
        StdlibEntry::Function(function) => (
            CompletionItemKind::FUNCTION,
            "function",
            function.deprecated.is_some(),
            function_documentation(name, function),
        ),
        StdlibEntry::Namespace(_) => {
            (CompletionItemKind::MODULE, "namespace", false, format!("`{name}` namespace"))
        }
        StdlibEntry::Constant(value) => (
            CompletionItemKind::CONSTANT,
            "constant",
            value.deprecated.is_some(),
            format!("`{name}` constant"),
        ),
        StdlibEntry::Property(value) => (
            CompletionItemKind::PROPERTY,
            "property",
            value.deprecated.is_some(),
            format!("`{name}` property"),
        ),
    };

    // Insert the bare name without placeholders. The stdlib model has
    // parameter types but no names, and signature help shows the real
    // signature once the user types `(`.
    CompletionItem {
        label: name.to_owned(),
        kind: Some(kind),
        detail: Some(detail.to_owned()),
        // Deprecated members sink below live ones in every list.
        sort_text: Some(format!("{}{name}", if is_deprecated { '2' } else { '1' })),
        deprecated: Some(is_deprecated),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        })),
        tags: is_deprecated.then(|| vec![CompletionItemTag::DEPRECATED]),
        insert_text: Some(name.to_owned()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        ..Default::default()
    }
}

fn function_documentation(name: &str, function: &StdlibFunction<'_>) -> String {
    let params =
        function.primary_signature().params.iter().map(param_label).collect::<Vec<_>>().join(", ");
    let overload_count = function.signatures.len() - 1;
    let overload_note = if overload_count > 0 {
        format!("\n\n_+{overload_count} overload(s)_")
    } else {
        String::new()
    };
    let deprecation_note = function.deprecated.as_ref().map_or_else(String::new, |deprecation| {
        format!("\n\n**Deprecated:** {}", deprecation.message)
    });
    let must_use_note = if function.is_must_use { "\n\n_must_use_" } else { "" };
    let pure_note = if function.is_pure { " \u{b7} _pure_" } else { "" };
    format!(
        "```luau\nfunction {name}({params})\n```{overload_note}{deprecation_note}{must_use_note}{pure_note}"
    )
}

const KEYWORD_COMPLETIONS: &[&str] = &[
    "and", "break", "continue", "do", "else", "elseif", "end", "false", "for", "function", "if",
    "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

#[cfg(test)]
mod tests {
    use ls_types::Range as LspRange;

    use super::*;
    use crate::fixtures::{document, roblox_document};

    fn items_at(document: &DocumentState, line: u32, utf16_column: u32) -> Vec<CompletionItem> {
        match completion(document, Position::new(line, utf16_column)) {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        }
    }

    fn keywords_of(items: &[CompletionItem]) -> Vec<&str> {
        items
            .iter()
            .filter(|item| item.kind == Some(CompletionItemKind::KEYWORD))
            .map(|item| item.label.as_str())
            .collect()
    }

    fn variables_of(items: &[CompletionItem]) -> Vec<&str> {
        items
            .iter()
            .filter(|item| item.kind == Some(CompletionItemKind::VARIABLE))
            .map(|item| item.label.as_str())
            .collect()
    }

    #[test]
    fn keywords_and_contextual_keywords_are_offered() {
        let items = items_at(&document(""), 0, 0);
        let keywords = keywords_of(&items);
        assert!(keywords.contains(&"while"), "{keywords:?}");
        assert!(keywords.contains(&"continue"), "{keywords:?}");
    }

    #[test]
    fn only_the_locals_in_scope_at_the_cursor_are_offered() {
        let text = "\
local a = 1
local function f(p) end
for i = 1, 2 do end
for k, v in pairs({}) do end
local t = {}
function t:m(q)
  do local inner = 1 end
  local a = 2

end
local later
";
        let document = document(text);
        // The blank line inside `t:m`.
        let items = items_at(&document, 8, 0);
        assert_eq!(variables_of(&items), ["a", "f", "t", "q"], "once each, without `self`");
    }

    #[test]
    fn a_block_s_locals_are_offered_up_to_what_ends_it() {
        let block = document(
            "\
do
  local before = 1

end
",
        );
        assert!(variables_of(&items_at(&block, 2, 0)).contains(&"before"), "before its `end`");
        assert!(!variables_of(&items_at(&block, 4, 0)).contains(&"before"), "after its `end`");
        let repeat = document("repeat local body = 1 until b");
        assert!(variables_of(&items_at(&repeat, 0, 29)).contains(&"body"), "in its condition");
        let unclosed = document(
            "\
function g(x)
  ",
        );
        assert!(variables_of(&items_at(&unclosed, 1, 2)).contains(&"x"), "with no `end` yet");
    }

    /// A string the line cut off is still being typed at its end, so its
    /// values complete there and replace all of it after the quote.
    #[test]
    fn an_unterminated_string_completes_its_values_at_its_end() {
        let text = "local p = game:GetService('Pla";
        let document = roblox_document(text);
        let utf16_column = index_to_offset(text.len());
        let items = items_at(&document, 0, utf16_column);
        let players = items.iter().find(|item| item.label == "Players").expect("Players offered");
        let Some(CompletionTextEdit::Edit(edit)) = &players.text_edit else {
            panic!("{players:?}");
        };
        let quote = index_to_offset(text.find('\'').expect("the quote") + 1);
        assert_eq!(
            edit.range,
            LspRange::new(Position::new(0, quote), Position::new(0, utf16_column))
        );
    }

    #[test]
    fn get_service_string_completes_services_ranked() {
        let text = "local p = game:GetService('')";
        let document = roblox_document(text);
        // Cursor between the quotes.
        let utf16_column = index_to_offset(text.find("('").expect("literal") + 2);
        let items = items_at(&document, 0, utf16_column);

        let players = items.iter().find(|item| item.label == "Players").expect("Players offered");
        assert!(
            players.sort_text.as_deref() == Some("0Players"),
            "common services rank first: {:?}",
            players.sort_text
        );
        let obscure = items
            .iter()
            .find(|item| item.label == "AccountService")
            .expect("full service list offered");
        assert_eq!(obscure.sort_text.as_deref(), Some("1AccountService"));
        let dead = items
            .iter()
            .find(|item| item.label == "PointsService")
            .expect("dead services offered (tagged)");
        assert_eq!(dead.sort_text.as_deref(), Some("2PointsService"));
        assert_eq!(dead.deprecated, Some(true));
        // Constant-value mode replaces identifier completion entirely.
        assert!(!items.iter().any(|item| item.label == "print"));
    }

    #[test]
    fn instance_new_string_completes_class_names() {
        let text = "local f = Instance.new('Fol')";
        let document = roblox_document(text);
        let utf16_column = index_to_offset(text.find("('").expect("literal") + 4);
        let items = items_at(&document, 0, utf16_column);
        assert!(items.iter().any(|item| item.label == "Folder"));
        assert!(
            !items.iter().any(|item| item.label == "BasePart"),
            "abstract classes are not creatable"
        );
    }

    #[test]
    fn enum_dot_completes_enum_types() {
        let text = "local m = Enum.";
        let document = roblox_document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert!(
            items.len() > 500,
            "expected the full generated enum tree, got {} items",
            items.len()
        );
        let material =
            items.iter().find(|item| item.label == "Material").expect("Enum.Material offered");
        assert_eq!(material.kind, Some(CompletionItemKind::MODULE));
    }

    #[test]
    fn enum_type_dot_completes_items() {
        let text = "local m = Enum.Material.";
        let document = roblox_document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert!(items.iter().any(|item| item.label == "Grass"));
        assert!(
            !items.iter().any(|item| item.label == "GetEnumItems"),
            "methods complete after ':', not '.'"
        );
    }

    #[test]
    fn enum_type_colon_completes_methods_only() {
        let text = "local items = Enum.Material:";
        let document = roblox_document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert!(items.iter().any(|item| item.label == "GetEnumItems"));
        assert!(items.iter().any(|item| item.label == "FromName"));
        assert!(
            !items.iter().any(|item| item.label == "Grass"),
            "items are dot members, not methods"
        );
    }

    #[test]
    fn deprecated_enum_item_completion_is_tagged() {
        let text = "local a = Enum.AlignType.";
        let document = roblox_document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        let parallel = items
            .iter()
            .find(|item| item.label == "Parallel")
            .expect("deprecated items still offered");
        assert_eq!(parallel.deprecated, Some(true));
    }

    #[test]
    fn completion_after_shaped_local_colon_offers_datamodel_methods() {
        let text = "\
local g = game
g:";
        let document = roblox_document(text);
        let items = items_at(&document, 1, 2);
        assert!(
            items.iter().any(|item| item.label == "GetService"),
            "DataModel methods expected on shaped local"
        );
        assert!(
            items.iter().any(|item| item.label == "FindFirstChild"),
            "extends-inherited Instance methods expected"
        );
    }

    #[test]
    fn completion_after_game_colon_offers_methods() {
        let text = "game:";
        let document = roblox_document(text);
        let items = items_at(&document, 0, 5);
        assert!(items.iter().any(|item| item.label == "GetService"));
        assert!(
            !items.iter().any(|item| item.label == "Workspace"),
            "properties complete after '.', not ':'"
        );
    }

    #[test]
    fn completion_ranks_deprecated_members_last() {
        let text = "local x = table.";
        let document = document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        let insert = items.iter().find(|item| item.label == "insert").expect("live member");
        let getn = items.iter().find(|item| item.label == "getn").expect("deprecated member");
        assert_eq!(insert.sort_text.as_deref(), Some("1insert"));
        assert_eq!(getn.sort_text.as_deref(), Some("2getn"));
        assert_eq!(getn.deprecated, Some(true));
    }

    #[test]
    fn completion_offers_nothing_inside_a_comment() {
        let text = "-- string.";
        let document = document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert!(
            items.is_empty(),
            "no completion belongs inside a comment, got {} items",
            items.len()
        );
    }

    #[test]
    fn completion_offers_nothing_inside_a_plain_string() {
        let text = "local s = \"string.\"";
        let document = document(text);
        // Just before the closing quote.
        let items = items_at(&document, 0, index_to_offset(text.len() - 1));
        assert!(
            items.is_empty(),
            "no completion belongs inside a non-constant string, got {} items",
            items.len()
        );
    }

    #[test]
    fn completion_refuses_members_of_a_shadowed_namespace() {
        let text = "\
local math = {}
math.";
        let document = document(text);
        let items = items_at(&document, 1, 5);
        assert!(
            !items.iter().any(|item| item.label == "floor"),
            "`math` is a local here, so the stdlib's members don't apply: {items:?}"
        );
    }

    #[test]
    fn constant_completion_in_a_long_string_replaces_only_its_interior() {
        let text = "collectgarbage([[c]])";
        let document = document(text);
        let items = items_at(&document, 0, 18);
        let collect = items
            .iter()
            .find(|item| item.label == "collect")
            .expect("constant values inside the string");
        let Some(CompletionTextEdit::Edit(edit)) = &collect.text_edit else {
            panic!("constant completion carries a text edit: {collect:?}");
        };
        assert_eq!(
            edit.range,
            LspRange::new(Position::new(0, 17), Position::new(0, 18)),
            "only the `c` inside `[[` and `]]` may be replaced, or the brackets break"
        );
    }

    #[test]
    fn completion_after_a_concat_offers_namespace_members() {
        let text = "local s = \"a\"..string.";
        let document = document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert!(
            items.iter().any(|item| item.label == "format"),
            "`..` ends the operand, so `string.` still completes its members: {:?}",
            keywords_of(&items)
        );
    }

    #[test]
    fn completion_after_a_member_dot_on_an_unknown_receiver_offers_no_keywords() {
        let text = "local result = make().";
        let document = document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert!(
            keywords_of(&items).is_empty(),
            "a keyword can't follow `.`: {:?}",
            keywords_of(&items)
        );
    }

    #[test]
    fn completion_offers_nothing_inside_an_unterminated_string() {
        let text = "local s = \"string.";
        let document = document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert_eq!(items.len(), 0, "the string is still being typed: {items:?}");
    }

    #[test]
    fn completion_after_a_keyword_shaped_partial_offers_namespace_members() {
        let text = "local s = string.for";
        let document = document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert!(items.iter().any(|item| item.label == "format"), "{items:?}");
    }
}
