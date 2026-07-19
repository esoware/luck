//! Completion provider. Offers stdlib globals, namespace members, and
//! visible scope locals.

use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;
use luck_semantic::stdlib_model::{
    EntryKind, StdlibArgKind, StdlibEntry, StdlibFunction, library_for,
};
use luck_token::{LuaVersion, Span, TokenKind};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemTag, CompletionParams, CompletionResponse,
    Documentation, InsertTextFormat, MarkupContent, MarkupKind,
};

use crate::backend::DocumentState;

#[must_use]
pub fn completion(doc: &DocumentState, params: &CompletionParams) -> Option<CompletionResponse> {
    let position = params.text_document_position.position;
    let offset = doc.line_index.offset(&doc.text, position);
    let environment = doc.target.stdlib_environment();
    let lib = library_for(doc.target.lua_version());

    let prefix = preceding_dotted_prefix(&doc.text, offset);
    let mut items: Vec<CompletionItem> = Vec::new();

    if let Some(namespace) = prefix {
        // After `string.` - only namespace members.
        if let Some(entry) = lib.lookup_str(&[namespace.as_str()]) {
            if let EntryKind::Namespace(members) = &entry.kind {
                for (name, member) in members {
                    if !member.available_in_luau(environment) {
                        continue;
                    }
                    items.push(item_for_entry(
                        name.as_str(),
                        member,
                        doc.target.lua_version(),
                    ));
                }
            }
        }
    } else {
        // Bare identifier: stdlib globals + scope locals + keywords.
        for (name, entry) in &lib.globals {
            if !entry.available_in_luau(environment) {
                continue;
            }
            items.push(item_for_entry(
                name.as_str(),
                entry,
                doc.target.lua_version(),
            ));
        }
        for (name, span) in visible_locals(&doc.parsed.block, offset) {
            let _ = span;
            items.push(CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::VARIABLE),
                ..Default::default()
            });
        }
        for kw in KEYWORD_COMPLETIONS {
            items.push(CompletionItem {
                label: (*kw).to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            });
        }
    }

    Some(CompletionResponse::Array(items))
}

/// Look at the bytes immediately before `offset`. If they form
/// `<name>.`, return `<name>` so we can complete its members.
fn preceding_dotted_prefix(source: &str, offset: u32) -> Option<String> {
    let bytes = source.as_bytes();
    if offset == 0 || offset as usize > bytes.len() {
        return None;
    }
    let mut idx = offset as usize;
    // Walk back over an in-progress partial identifier.
    while idx > 0 && is_ident_byte(bytes[idx - 1]) {
        idx -= 1;
    }
    if idx == 0 || bytes[idx - 1] != b'.' {
        return None;
    }
    idx -= 1;
    let end = idx;
    while idx > 0 && is_ident_byte(bytes[idx - 1]) {
        idx -= 1;
    }
    if end == idx {
        return None;
    }
    std::str::from_utf8(&bytes[idx..end])
        .ok()
        .map(str::to_string)
}

fn is_ident_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

fn item_for_entry(name: &str, entry: &StdlibEntry, _version: LuaVersion) -> CompletionItem {
    let kind = match &entry.kind {
        EntryKind::Function(_) => CompletionItemKind::FUNCTION,
        EntryKind::Namespace(_) => CompletionItemKind::MODULE,
        EntryKind::Constant(_) => CompletionItemKind::CONSTANT,
        EntryKind::Property(_) => CompletionItemKind::PROPERTY,
    };

    let (deprecated, doc_md) = match &entry.kind {
        EntryKind::Function(func) => (
            func.deprecated.is_some(),
            format_function_doc(
                name,
                func,
                matches!(
                    entry.luau_tier,
                    luck_semantic::stdlib_model::LuauTier::Roblox
                ),
            ),
        ),
        EntryKind::Namespace(_) => (false, Some(format!("`{name}` namespace"))),
        EntryKind::Constant(v) => (v.deprecated.is_some(), Some(format!("`{name}` constant"))),
        EntryKind::Property(v) => (v.deprecated.is_some(), Some(format!("`{name}` property"))),
    };

    let detail = match &entry.kind {
        EntryKind::Function(_) => Some("function".to_string()),
        EntryKind::Namespace(_) => Some("namespace".to_string()),
        EntryKind::Constant(_) => Some("constant".to_string()),
        EntryKind::Property(_) => Some("property".to_string()),
    };

    // Insert just the bare name - no paren, no placeholders. The stdlib
    // model has parameter *types* but not *names*, and synthesizing fake
    // names like `xpcall(fn, fn)` is more annoying than helpful. The
    // user types `(`, signatureHelp shows the real signature.
    CompletionItem {
        label: name.to_string(),
        kind: Some(kind),
        detail,
        deprecated: Some(deprecated),
        documentation: doc_md.map(|md| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            })
        }),
        tags: if deprecated {
            Some(vec![CompletionItemTag::DEPRECATED])
        } else {
            None
        },
        insert_text: Some(name.to_string()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        ..Default::default()
    }
}

fn format_function_doc(name: &str, func: &StdlibFunction, is_roblox: bool) -> Option<String> {
    let params = func
        .params
        .iter()
        .map(|p| match &p.kind {
            StdlibArgKind::Vararg => "...".to_string(),
            kind => {
                let label = match kind {
                    StdlibArgKind::Any => "any",
                    StdlibArgKind::Bool => "bool",
                    StdlibArgKind::Number => "number",
                    StdlibArgKind::String => "string",
                    StdlibArgKind::Function => "function",
                    StdlibArgKind::Table => "table",
                    StdlibArgKind::Nil => "nil",
                    StdlibArgKind::Display(d) => d.as_str(),
                    StdlibArgKind::Constant(_) => "constant",
                    StdlibArgKind::Vararg => unreachable!(),
                };
                if p.required {
                    label.to_string()
                } else {
                    format!("{label}?")
                }
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!("```lua\nfunction {name}({params})\n```");
    if let Some(dep) = &func.deprecated {
        out.push_str(&format!("\n\n**Deprecated:** {}", dep.message));
    }
    if func.must_use {
        out.push_str("\n\n_must_use_");
    }
    if func.is_pure {
        out.push_str(" · _pure_");
    }
    if is_roblox {
        out.push_str(" · _Roblox-only_");
    }
    Some(out)
}

const KEYWORD_COMPLETIONS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Collect every local-binding or parameter name whose declaration site
/// precedes `offset`. This is intentionally lexical-only - we don't try
/// to mirror the scope walk because completion is best-effort.
#[must_use]
fn visible_locals(block: &Block, offset: u32) -> Vec<(String, Span)> {
    let mut collector = LocalCollector {
        offset,
        out: Vec::new(),
    };
    collector.visit_block(block);
    collector.out
}

struct LocalCollector {
    offset: u32,
    out: Vec<(String, Span)>,
}

impl Visitor for LocalCollector {
    fn visit_statement(&mut self, stmt: &luck_ast::Statement) {
        use luck_ast::Statement;
        match stmt {
            Statement::LocalAssignment(local) => {
                for name_token in local.names.iter() {
                    if name_token.name.span.end <= self.offset {
                        if let TokenKind::Identifier(ident) = &name_token.name.kind {
                            self.out.push((ident.to_string(), name_token.name.span));
                        }
                    }
                }
            }
            Statement::LocalFunction(local_fn) if local_fn.name.span.end <= self.offset => {
                if let TokenKind::Identifier(ident) = &local_fn.name.kind {
                    self.out.push((ident.to_string(), local_fn.name.span));
                }
            }
            Statement::NumericFor(nfor) if nfor.name.span.end <= self.offset => {
                if let TokenKind::Identifier(ident) = &nfor.name.kind {
                    self.out.push((ident.to_string(), nfor.name.span));
                }
            }
            Statement::GenericFor(gfor) => {
                for binding in gfor.names.iter() {
                    if binding.name.span.end <= self.offset {
                        if let TokenKind::Identifier(ident) = &binding.name.kind {
                            self.out.push((ident.to_string(), binding.name.span));
                        }
                    }
                }
            }
            _ => {}
        }
        self.walk_statement(stmt);
    }

    fn visit_function_body(&mut self, body: &luck_ast::shared::FunctionBody) {
        for param in body.params.iter() {
            if param.name.span.end <= self.offset {
                if let TokenKind::Identifier(ident) = &param.name.kind {
                    self.out.push((ident.to_string(), param.name.span));
                }
            }
        }
        self.walk_function_body(body);
    }
}
