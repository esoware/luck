//! Known limitation: a literal receiver mid-typing (`("x"):` with nothing
//! after the colon) gets no member completion. The dangling colon doesn't
//! parse, so there's no AST to resolve the receiver from.

use luck_ast::{FunctionArgs, FunctionCall, Literal};
use luck_semantic::{StdlibEntry, StdlibFunction};
use luck_token::{Span, TokenKind, index_to_offset};
use rustc_hash::FxHashSet;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemTag, CompletionResponse, CompletionTextEdit,
    Documentation, InsertTextFormat, MarkupContent, MarkupKind, Position, TextEdit,
};

use crate::document::DocumentState;
use crate::providers::cursor::{
    TextKind, dotted_chain_ending_tokens, find_string_arg_at, text_kind_at,
};
use crate::providers::stdlib_render::param_label;

pub(crate) fn completion(document: &DocumentState, position: Position) -> CompletionResponse {
    let offset = document.offset(position);
    if let Some(items) = constant_value_completions(document, offset) {
        return CompletionResponse::Array(items);
    }
    if text_kind_at(document, offset) != TextKind::Code {
        return CompletionResponse::Array(Vec::new());
    }
    let items = member_access_before(document, offset).map_or_else(
        || bare_name_completions(document, offset),
        |access| member_completions(document, &access, offset),
    );
    CompletionResponse::Array(items)
}

fn bare_name_completions(document: &DocumentState, offset: u32) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = document
        .semantic
        .stdlib()
        .globals
        .iter()
        .map(|(name, entry)| item_for_entry(name.as_str(), entry))
        .collect();
    // Every local or parameter declared before the cursor, whether or not it's
    // still in scope there, because completion is best-effort. The implicit
    // `self` has an empty span, so it fails the text check.
    for symbol in &document.semantic.scope_tree.symbols {
        let span = symbol.definition_span;
        if span.end <= offset && span.text(document.text()) == symbol.name.as_str() {
            items.push(CompletionItem {
                label: symbol.name.to_string(),
                kind: Some(CompletionItemKind::VARIABLE),
                ..Default::default()
            });
        }
    }
    for keyword in KEYWORD_COMPLETIONS {
        if TokenKind::is_reserved(keyword, document.lua_version) {
            items.push(CompletionItem {
                label: (*keyword).to_owned(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            });
        }
    }
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
    let tree = &document.semantic.scope_tree;
    // A reference at the root is exact. Text mid-typing often has none, so
    // that case falls back to the nearest declaration before the cursor.
    let (root_local, reaches_host_global) = tree.references_by_span.get(root_span).map_or_else(
        || {
            let local = document.semantic.nearest_local(root, offset).map(|symbol| symbol.id);
            (local, local.is_none())
        },
        |&reference_id| {
            (tree.reference(reference_id).resolved, tree.reaches_host_global(root, *root_span))
        },
    );

    let stdlib = document.semantic.stdlib();
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut push = |name: &str, member: &StdlibEntry| {
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
            push(name.as_str(), member);
        }
    }
    // A shaped local: `local f = io.open(...); f:` offers the file methods,
    // `local g = game; g:` the DataModel ones.
    let shape = match (stdlib_entry, root_local, segments.as_slice()) {
        (Some(entry), _, _) => entry.shape(),
        (None, Some(symbol_id), [_]) => document.semantic.symbol_shapes.get(&symbol_id),
        (None, _, _) => None,
    };
    if let Some(shape_members) = shape.and_then(|name| stdlib.shapes.get(name.as_str())) {
        for (name, member) in &shape_members.members {
            push(name.as_str(), member);
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
    let site = find_string_arg_at(&document.parsed.block, offset)?;
    let callee = document.semantic.resolve_callee(site.call)?;
    let StdlibEntry::Function(function) = callee.entry() else {
        return None;
    };
    let allowed_sets = function.allowed_constants(site.arg_index, call_arg_count(site.call))?;
    // Replace the literal's interior, so the client filters on whatever is
    // typed inside the delimiters.
    let interior = document.range(string_interior(site.literal));
    let mut seen_values: FxHashSet<&str> = FxHashSet::default();
    let mut items: Vec<CompletionItem> = Vec::new();
    for constant in allowed_sets.into_iter().flatten() {
        if !seen_values.insert(constant.value.as_str()) {
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
            label: constant.value.to_string(),
            kind: Some(CompletionItemKind::ENUM_MEMBER),
            sort_text: Some(format!("{rank}{}", constant.value)),
            filter_text: Some(constant.value.to_string()),
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
                new_text: constant.value.to_string(),
            })),
            ..Default::default()
        });
    }
    Some(items)
}

const fn call_arg_count(call: &FunctionCall) -> usize {
    match &call.args {
        FunctionArgs::Parenthesized { args, .. } => args.len(),
        FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => 1,
    }
}

/// The part of a string literal between its delimiters. A long bracket's
/// opener is `[`, any number of `=`, then `[`, and its closer mirrors it.
fn string_interior(literal: &Literal) -> Span {
    let delimiter_width =
        index_to_offset(literal.text.strip_prefix('[').map_or(1, |after_bracket| {
            after_bracket.bytes().take_while(|&b| b == b'=').count().saturating_add(2)
        }));
    Span::new(
        literal.span.start.saturating_add(delimiter_width),
        literal.span.end.saturating_sub(delimiter_width),
    )
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
    let tokens = document.tokens();
    let before_cursor = tokens.get(..tokens.partition_point(|token| token.span.start < offset))?;
    let (last, before_last) = before_cursor.split_last()?;
    // The word in progress can lex as a keyword: `string.for` is on its way
    // to `format`.
    let is_partial_word =
        offset <= last.span.end && TokenKind::is_identifier_shaped(last.span.text(document.text()));
    let (separator, before_separator) =
        if is_partial_word { before_last.split_last()? } else { (last, before_last) };
    let is_colon = matches!(separator.kind, TokenKind::Colon);
    if !is_colon && !matches!(separator.kind, TokenKind::Dot) {
        return None;
    }
    Some(MemberAccess { receiver: dotted_chain_ending_tokens(before_separator), is_colon })
}

fn item_for_entry(name: &str, entry: &StdlibEntry) -> CompletionItem {
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

fn function_documentation(name: &str, function: &StdlibFunction) -> String {
    let params =
        function.primary_signature().params.iter().map(param_label).collect::<Vec<_>>().join(", ");
    let overload_count = function.signatures.len().saturating_sub(1);
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
        "```lua\nfunction {name}({params})\n```{overload_note}{deprecation_note}{must_use_note}{pure_note}"
    )
}

/// Every target's keywords. A target offers only those `is_reserved` admits.
const KEYWORD_COMPLETIONS: &[&str] = &[
    "and", "break", "continue", "do", "else", "elseif", "end", "false", "for", "function",
    "global", "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true",
    "until", "while",
];

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use tower_lsp::lsp_types::Range as LspRange;

    use super::*;

    fn items_at(document: &DocumentState, line: u32, utf16_column: u32) -> Vec<CompletionItem> {
        match completion(document, Position::new(line, utf16_column)) {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        }
    }

    fn document_for(text: &str, target: LuaTarget) -> DocumentState {
        DocumentState::new(text.to_owned(), 1, target, FxHashSet::default())
    }

    fn roblox_document(text: &str) -> DocumentState {
        document_for(text, LuaTarget::LuauRoblox)
    }

    fn lua54_document(text: &str) -> DocumentState {
        document_for(text, LuaTarget::Lua54)
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
    fn keywords_follow_the_target() {
        for (target, expect_goto, expect_global, expect_continue) in [
            (LuaTarget::Lua51, false, false, false),
            (LuaTarget::Lua52, true, false, false),
            (LuaTarget::Lua53, true, false, false),
            (LuaTarget::Lua54, true, false, false),
            (LuaTarget::Lua55, true, true, false),
            (LuaTarget::Luau, false, false, true),
            (LuaTarget::LuauRoblox, false, false, true),
        ] {
            let document = document_for("", target);
            let items = items_at(&document, 0, 0);
            let keywords = keywords_of(&items);
            assert!(keywords.contains(&"while"), "{target:?}: {keywords:?}");
            assert_eq!(keywords.contains(&"goto"), expect_goto, "{target:?}: {keywords:?}");
            assert_eq!(keywords.contains(&"global"), expect_global, "{target:?}: {keywords:?}");
            assert_eq!(keywords.contains(&"continue"), expect_continue, "{target:?}: {keywords:?}");
        }
    }

    #[test]
    fn locals_before_the_cursor_are_offered_without_implicit_self() {
        let text = "local a = 1\nlocal function f(p) end\nfor i = 1, 2 do end\n\
                    for k, v in pairs({}) do end\nlocal t = {}\nfunction t:m(q)\n\nend\nlocal later\n";
        let document = lua54_document(text);
        // The blank line inside `t:m`.
        let items = items_at(&document, 6, 0);
        let variables = variables_of(&items);
        for name in ["a", "f", "p", "i", "k", "v", "t", "q"] {
            assert!(variables.contains(&name), "{name}: {variables:?}");
        }
        assert!(!variables.contains(&"self"), "{variables:?}");
        assert!(!variables.contains(&"later"), "{variables:?}");
    }

    #[test]
    fn named_vararg_is_offered_on_lua55() {
        let text = "local function f(...args)\n\nend\n";
        let document = document_for(text, LuaTarget::Lua55);
        let items = items_at(&document, 1, 0);
        assert!(variables_of(&items).contains(&"args"), "{items:?}");
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
    fn completion_after_shaped_local_colon_offers_file_methods() {
        let text = "local f = io.open('x')\nf:";
        let document = lua54_document(text);
        let items = items_at(&document, 1, 2);
        assert!(
            items.iter().any(|item| item.label == "read"),
            "file methods expected: {:?}",
            items.iter().map(|item| &item.label).collect::<Vec<_>>()
        );
        assert!(items.iter().any(|item| item.label == "seek"));
    }

    #[test]
    fn completion_after_shaped_local_colon_offers_datamodel_methods() {
        let text = "local g = game\ng:";
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
        let text = "local x = math.";
        let document = lua54_document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        let floor = items.iter().find(|item| item.label == "floor").expect("live member");
        let pow = items.iter().find(|item| item.label == "pow").expect("deprecated compat member");
        assert_eq!(floor.sort_text.as_deref(), Some("1floor"));
        assert_eq!(pow.sort_text.as_deref(), Some("2pow"));
        assert_eq!(pow.deprecated, Some(true));
    }

    #[test]
    fn completion_offers_nothing_inside_a_comment() {
        let text = "-- string.";
        let document = lua54_document(text);
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
        let document = lua54_document(text);
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
        let text = "local math = {}\nmath.";
        let document = lua54_document(text);
        let items = items_at(&document, 1, 5);
        assert!(
            !items.iter().any(|item| item.label == "floor"),
            "`math` is a local here, so the stdlib's members don't apply: {items:?}"
        );
    }

    #[test]
    fn constant_completion_in_a_long_string_replaces_only_its_interior() {
        let text = "collectgarbage([[c]])";
        let document = lua54_document(text);
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
        let document = lua54_document(text);
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
        let document = lua54_document(text);
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
        let document = lua54_document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert_eq!(items.len(), 0, "the string is still being typed: {items:?}");
    }

    #[test]
    fn completion_after_a_keyword_shaped_partial_offers_namespace_members() {
        let text = "local s = string.for";
        let document = lua54_document(text);
        let items = items_at(&document, 0, index_to_offset(text.len()));
        assert!(items.iter().any(|item| item.label == "format"), "{items:?}");
    }
}
