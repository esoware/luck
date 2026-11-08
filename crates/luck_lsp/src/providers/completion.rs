//! Completion provider.
//!
//! Offers stdlib globals, namespace members, locals declared before the
//! cursor, keywords, shaped-local members, and constant-set values inside a
//! string argument whose parameter is constant-typed (service names, class
//! names, collectgarbage options).
//!
//! Known limitation: a literal receiver mid-typing (`("x"):` with nothing
//! after the colon) gets no member completion. The dangling colon doesn't
//! parse, so there's no AST to resolve the receiver from.

use core::ops::Range;

use luck_ast::{Block, FunctionArgs, FunctionBody, FunctionCall, Statement, Visitor};
use luck_semantic::{StdlibEntry, StdlibFunction};
use luck_token::{Span, Token};
use rustc_hash::FxHashSet;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemTag, CompletionResponse, CompletionTextEdit,
    Documentation, InsertTextFormat, MarkupContent, MarkupKind, Position, TextEdit,
};

use crate::document::DocumentState;
use crate::providers::cursor::find_string_arg_at;
use crate::stdlib_render::param_label;

pub(crate) fn completion(document: &DocumentState, position: Position) -> CompletionResponse {
    let offset = document.offset(position);
    if let Some(items) = constant_value_completions(document, offset) {
        return CompletionResponse::Array(items);
    }

    let stdlib = document.semantic.stdlib();
    let mut items: Vec<CompletionItem> = Vec::new();
    if let Some(prefix) = preceding_member_prefix(document.text(), offset) {
        // After `string.`, `Enum.Material.`, or `Enum.Material:`, offer the
        // prefix's members: non-methods after a dot, methods after a colon.
        let mut push = |name: &str, member: &StdlibEntry| {
            let is_method = matches!(member, StdlibEntry::Function(function) if function.is_method);
            if is_method == prefix.is_colon {
                items.push(item_for_entry(name, member));
            }
        };
        let shape = if let Some(entry) = stdlib.lookup(&prefix.segments) {
            if let StdlibEntry::Namespace(namespace) = entry
                && !prefix.is_colon
            {
                for (name, member) in &namespace.members {
                    push(name.as_str(), member);
                }
            }
            entry.shape()
        } else if let [root] = prefix.segments.as_slice() {
            // A shaped local: `local f = io.open(...); f:` offers the
            // file methods, `local g = game; g:` the DataModel ones. It
            // resolves to the nearest declaration before the cursor, because
            // text mid-typing has no reference to resolve by span.
            document.semantic.shape_of_nearest_local(root, offset)
        } else {
            None
        };
        if let Some(shape_members) = shape.and_then(|name| stdlib.shapes.get(name.as_str())) {
            for (name, member) in &shape_members.members {
                push(name.as_str(), member);
            }
        }
    } else {
        // Bare identifier: stdlib globals + scope locals + keywords.
        for (name, entry) in &stdlib.globals {
            items.push(item_for_entry(name.as_str(), entry));
        }
        for name in visible_locals(&document.parsed.block, offset) {
            items.push(CompletionItem {
                label: name.to_owned(),
                kind: Some(CompletionItemKind::VARIABLE),
                ..Default::default()
            });
        }
        for keyword in KEYWORD_COMPLETIONS {
            items.push(CompletionItem {
                label: (*keyword).to_owned(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            });
        }
    }

    CompletionResponse::Array(items)
}

/// Offers the allowed values when the cursor sits in a constant-typed string
/// argument.
///
/// The literal must be a positional argument whose parameter is constant-typed
/// in every signature matching the call's arity. `None` falls through to
/// ordinary identifier completion.
fn constant_value_completions(
    document: &DocumentState,
    offset: u32,
) -> Option<Vec<CompletionItem>> {
    let site = find_string_arg_at(&document.parsed.block, offset)?;
    let callee = document.semantic.resolve_callee(site.call)?;
    let StdlibEntry::Function(function) = callee.entry else {
        return None;
    };
    let allowed_sets = function.allowed_constants(site.arg_index, call_arg_count(site.call))?;
    // Replace the literal's interior, so the client filters on whatever is
    // typed inside the quotes.
    let interior = document.range(Span::new(
        site.literal_span.start.saturating_add(1),
        site.literal_span.end.saturating_sub(1),
    ));
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

struct MemberPrefix<'source> {
    /// Dotted path before the final separator, root first
    /// (`Enum.Material.` -> `["Enum", "Material"]`).
    segments: Vec<&'source str>,
    /// Final separator was `:`, so complete methods instead of members.
    is_colon: bool,
}

/// The dotted path right before `offset` when it ends in `.` or `:`,
/// optionally followed by a partial name (`a.b.` / `a.b:fo`).
fn preceding_member_prefix(source: &str, offset: u32) -> Option<MemberPrefix<'_>> {
    let before_cursor = source.get(Range::<usize>::from(Span::new(0, offset)))?;
    // Walk back over an in-progress partial identifier.
    let before_partial = before_cursor.trim_end_matches(is_identifier_char);
    let (receiver, is_colon) = match before_partial.strip_suffix(':') {
        Some(receiver) => (receiver, true),
        None => (before_partial.strip_suffix('.')?, false),
    };
    let mut segments = Vec::new();
    let mut remaining = Some(receiver);
    while let Some(path) = remaining {
        let before_segment = path.trim_end_matches(is_identifier_char);
        let segment = path.strip_prefix(before_segment)?;
        if segment.is_empty() {
            return None;
        }
        segments.push(segment);
        // Only dots continue the path; the colon was the final separator.
        remaining = before_segment.strip_suffix('.');
    }
    segments.reverse();
    Some(MemberPrefix { segments, is_colon })
}

const fn is_identifier_char(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
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

const KEYWORD_COMPLETIONS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Every local or parameter name declared before `offset`, whether or not
/// it's still in scope there. Completion is best-effort and doesn't follow
/// scopes.
fn visible_locals(block: &Block, offset: u32) -> Vec<&str> {
    let mut collector = LocalCollector { offset, names: Vec::new() };
    collector.visit_block(block);
    collector.names
}

struct LocalCollector<'ast> {
    offset: u32,
    names: Vec<&'ast str>,
}

impl<'ast> LocalCollector<'ast> {
    fn record(&mut self, token: &'ast Token) {
        if token.span.end <= self.offset
            && let Some(name) = token.identifier()
        {
            self.names.push(name);
        }
    }
}

impl<'ast> Visitor<'ast> for LocalCollector<'ast> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                for name_token in local.names.iter() {
                    self.record(&name_token.name);
                }
            }
            Statement::LocalFunction(local_function) => self.record(&local_function.name),
            Statement::NumericFor(numeric_for) => self.record(&numeric_for.name),
            Statement::GenericFor(generic_for) => {
                for binding in generic_for.names.iter() {
                    self.record(&binding.name);
                }
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::FunctionDecl(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_statement(stmt);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        for param in body.params.iter() {
            self.record(&param.name);
        }
        self.walk_function_body(body);
    }
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use luck_token::index_to_offset;

    use super::*;

    fn items_at(document: &DocumentState, line: u32, utf16_column: u32) -> Vec<CompletionItem> {
        match completion(document, Position::new(line, utf16_column)) {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        }
    }

    fn roblox_document(text: &str) -> DocumentState {
        DocumentState::new(text.to_owned(), 1, LuaTarget::LuauRoblox)
    }

    fn lua54_document(text: &str) -> DocumentState {
        DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54)
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
}
