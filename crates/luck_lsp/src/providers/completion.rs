//! Completion provider. Offers stdlib globals, namespace members,
//! visible scope locals, shaped-local members, and constant-set values
//! inside a string argument whose parameter is constant-typed (service
//! names, class names, collectgarbage options).
//!
//! Known limitation: a literal receiver mid-typing (`("x"):` with
//! nothing after the colon) is not completed - the dangling colon does
//! not parse and there is no error-tolerant recovery yet. Parsed
//! literal-receiver code resolves fine everywhere else (hover, lints,
//! semantic tokens).

use luck_ast::expr::{FunctionArgs, FunctionCall};
use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;
use luck_semantic::stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibEntry, StdlibFunction, library_for,
};
use luck_token::TokenKind;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemTag, CompletionParams, CompletionResponse,
    CompletionTextEdit, Documentation, InsertTextFormat, MarkupContent, MarkupKind, TextEdit,
};

use crate::backend::DocumentState;
use crate::providers::cursor::find_string_arg_at;
use crate::stdlib_render::param_label;

#[must_use]
pub fn completion(doc: &DocumentState, params: &CompletionParams) -> Option<CompletionResponse> {
    let position = params.text_document_position.position;
    let offset = doc.line_index.offset(&doc.text, position);
    let lib = library_for(doc.target.lua_version(), doc.target.stdlib_environment());

    if let Some(items) = constant_value_completions(doc, offset) {
        return Some(CompletionResponse::Array(items));
    }

    let prefix = preceding_member_prefix(&doc.text, offset);
    let mut items: Vec<CompletionItem> = Vec::new();

    if let Some(prefix) = prefix {
        // After `string.` / `Enum.Material.` / `Enum.Material:` - only
        // members of the resolved prefix. Dot access offers namespace
        // members plus non-method shape members; colon access offers
        // methods only.
        let segments: Vec<&str> = prefix.segments.iter().map(String::as_str).collect();
        let mut push = |name: &str, member: &StdlibEntry| {
            let is_method = matches!(member, StdlibEntry::Function(func) if func.is_method);
            if is_method == prefix.is_colon {
                items.push(item_for_entry(name, member));
            }
        };
        if let Some(entry) = lib.lookup_str(&segments) {
            let shape = match entry {
                StdlibEntry::Namespace(namespace) => {
                    if !prefix.is_colon {
                        for (name, member) in &namespace.members {
                            push(name.as_str(), member);
                        }
                    }
                    namespace.shape.as_ref()
                }
                StdlibEntry::Constant(value) | StdlibEntry::Property(value) => value.shape.as_ref(),
                StdlibEntry::Function(_) => None,
            };
            if let Some(shape_members) = shape.and_then(|name| lib.shapes.get(name.as_str())) {
                for (name, member) in &shape_members.members {
                    push(name.as_str(), member);
                }
            }
        } else if segments.len() == 1
            && let Some(shape_name) = doc.semantic.shape_of_nearest_local(segments[0], offset)
            && let Some(shape_members) = lib.shapes.get(shape_name.as_str())
        {
            // A shaped local: `local f = io.open(...); f:` offers the
            // file methods, `local g = game; g:` the DataModel ones.
            // Lexical nearest-declaration resolution - mid-typing text
            // has no reference to resolve span-exactly.
            for (name, member) in &shape_members.members {
                push(name.as_str(), member);
            }
        }
    } else {
        // Bare identifier: stdlib globals + scope locals + keywords.
        for (name, entry) in &lib.globals {
            items.push(item_for_entry(name.as_str(), entry));
        }
        for name in visible_locals(&doc.parsed.block, offset) {
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

/// When the cursor sits inside a string literal that is a positional
/// argument whose parameter is constant-typed in every signature
/// matching the call's arity, offer the allowed values. `None` falls
/// through to ordinary identifier completion.
fn constant_value_completions(doc: &DocumentState, offset: u32) -> Option<Vec<CompletionItem>> {
    let site = find_string_arg_at(&doc.parsed.block, offset)?;
    let (_, resolved) = doc.semantic.resolve_callee(site.call)?;
    let StdlibEntry::Function(func) = resolved.entry else {
        return None;
    };
    let arg_count = call_arg_count(site.call);
    let mut allowed: Vec<&StdlibConstant> = Vec::new();
    let mut constrained = false;
    let mut unconstrained = false;
    for sig in func.matching_signatures(arg_count) {
        match sig.params.get(site.arg_index).map(|param| &param.kind) {
            Some(StdlibArgKind::Constant(values)) => {
                constrained = true;
                allowed.extend(values.iter());
            }
            _ => unconstrained = true,
        }
    }
    if !constrained || unconstrained {
        return None;
    }
    // The replacement range is the literal's interior; typing a prefix
    // inside the quotes narrows client-side filtering against it.
    let interior = doc.line_index.range(
        &doc.text,
        site.literal_span.start + 1,
        site.literal_span.end - 1,
    );
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut items: Vec<CompletionItem> = Vec::new();
    for constant in allowed {
        if !seen.insert(constant.value.as_str()) {
            continue;
        }
        let deprecated = constant.deprecated.is_some();
        // Rank: curated common values, then the rest, deprecated last.
        let rank = if deprecated {
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
            deprecated: Some(deprecated),
            tags: deprecated.then(|| vec![CompletionItemTag::DEPRECATED]),
            documentation: constant.deprecated.as_ref().map(|dep| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("**Deprecated:** {}", dep.message),
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

fn call_arg_count(call: &FunctionCall) -> usize {
    match &call.args {
        FunctionArgs::Parenthesized { args, .. } => args.len(),
        FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => 1,
    }
}

struct MemberPrefix {
    /// Dotted path before the final separator, outermost first
    /// (`Enum.Material.` -> `["Enum", "Material"]`).
    segments: Vec<String>,
    /// Final separator was `:` - complete methods instead of members.
    is_colon: bool,
}

/// Look at the bytes immediately before `offset`. If they form a dotted
/// path ending in `.` or `:` (`a.b.` / `a.b:`), return the path so we
/// can complete its members.
fn preceding_member_prefix(source: &str, offset: u32) -> Option<MemberPrefix> {
    let bytes = source.as_bytes();
    if offset == 0 || offset as usize > bytes.len() {
        return None;
    }
    let mut idx = offset as usize;
    // Walk back over an in-progress partial identifier.
    while idx > 0 && is_ident_byte(bytes[idx - 1]) {
        idx -= 1;
    }
    if idx == 0 || (bytes[idx - 1] != b'.' && bytes[idx - 1] != b':') {
        return None;
    }
    let is_colon = bytes[idx - 1] == b':';
    idx -= 1;
    let mut segments: Vec<String> = Vec::new();
    loop {
        let end = idx;
        while idx > 0 && is_ident_byte(bytes[idx - 1]) {
            idx -= 1;
        }
        if end == idx {
            return None;
        }
        segments.push(std::str::from_utf8(&bytes[idx..end]).ok()?.to_string());
        // Only dots continue the path; the colon was the final separator.
        if idx == 0 || bytes[idx - 1] != b'.' {
            break;
        }
        idx -= 1;
    }
    segments.reverse();
    Some(MemberPrefix { segments, is_colon })
}

fn is_ident_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

fn item_for_entry(name: &str, entry: &StdlibEntry) -> CompletionItem {
    let kind = match entry {
        StdlibEntry::Function(_) => CompletionItemKind::FUNCTION,
        StdlibEntry::Namespace(_) => CompletionItemKind::MODULE,
        StdlibEntry::Constant(_) => CompletionItemKind::CONSTANT,
        StdlibEntry::Property(_) => CompletionItemKind::PROPERTY,
    };

    let (deprecated, doc_md) = match entry {
        StdlibEntry::Function(func) => (func.deprecated.is_some(), format_function_doc(name, func)),
        StdlibEntry::Namespace(_) => (false, Some(format!("`{name}` namespace"))),
        StdlibEntry::Constant(v) => (v.deprecated.is_some(), Some(format!("`{name}` constant"))),
        StdlibEntry::Property(v) => (v.deprecated.is_some(), Some(format!("`{name}` property"))),
    };

    let detail = match entry {
        StdlibEntry::Function(_) => Some("function".to_string()),
        StdlibEntry::Namespace(_) => Some("namespace".to_string()),
        StdlibEntry::Constant(_) => Some("constant".to_string()),
        StdlibEntry::Property(_) => Some("property".to_string()),
    };

    // Insert just the bare name - no paren, no placeholders. The stdlib
    // model has parameter *types* but not *names*, and synthesizing fake
    // names like `xpcall(fn, fn)` is more annoying than helpful. The
    // user types `(`, signatureHelp shows the real signature.
    CompletionItem {
        label: name.to_string(),
        kind: Some(kind),
        detail,
        // Deprecated members sink below live ones in every list.
        sort_text: Some(format!("{}{name}", if deprecated { '2' } else { '1' })),
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

fn format_function_doc(name: &str, func: &StdlibFunction) -> Option<String> {
    let params = func
        .primary_signature()
        .params
        .iter()
        .map(param_label)
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!("```lua\nfunction {name}({params})\n```");
    if func.signatures.len() > 1 {
        out.push_str(&format!("\n\n_+{} overload(s)_", func.signatures.len() - 1));
    }
    if let Some(dep) = &func.deprecated {
        out.push_str(&format!("\n\n**Deprecated:** {}", dep.message));
    }
    if func.must_use {
        out.push_str("\n\n_must_use_");
    }
    if func.is_pure {
        out.push_str(" · _pure_");
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
fn visible_locals(block: &Block, offset: u32) -> Vec<String> {
    let mut collector = LocalCollector {
        offset,
        out: Vec::new(),
    };
    collector.visit_block(block);
    collector.out
}

struct LocalCollector {
    offset: u32,
    out: Vec<String>,
}

impl LocalCollector {
    fn record(&mut self, token: &luck_token::Token) {
        if token.span.end <= self.offset
            && let TokenKind::Identifier(ident) = &token.kind
        {
            self.out.push(ident.to_string());
        }
    }
}

impl<'ast> Visitor<'ast> for LocalCollector {
    fn visit_statement(&mut self, stmt: &'ast luck_ast::Statement) {
        use luck_ast::Statement;
        match stmt {
            Statement::LocalAssignment(local) => {
                for name_token in local.names.iter() {
                    self.record(&name_token.name);
                }
            }
            Statement::LocalFunction(local_fn) => self.record(&local_fn.name),
            Statement::NumericFor(nfor) => self.record(&nfor.name),
            Statement::GenericFor(gfor) => {
                for binding in gfor.names.iter() {
                    self.record(&binding.name);
                }
            }
            _ => {}
        }
        self.walk_statement(stmt);
    }

    fn visit_function_body(&mut self, body: &'ast luck_ast::shared::FunctionBody) {
        for param in body.params.iter() {
            self.record(&param.name);
        }
        self.walk_function_body(body);
    }
}
