//! Hover provider. Looks up the identifier under the cursor in the rich
//! stdlib model and renders signatures (all overloads), deprecation, and
//! must-use markers as a markdown panel.

use luck_semantic::stdlib_model::{StdlibArgKind, StdlibEntry, StdlibParam, library_for};
use luck_token::LuaVersion;
use tower_lsp::lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind, Range};

use crate::backend::DocumentState;
use crate::providers::cursor::{CursorTarget, find_target_at};
use crate::stdlib_render::param_label;

#[must_use]
pub fn hover(doc: &DocumentState, params: &HoverParams) -> Option<Hover> {
    let position = params.text_document_position_params.position;
    let offset = doc.line_index.offset(&doc.text, position);
    let target = find_target_at(&doc.parsed.block, offset)?;
    let path = target.path();
    // Root-span-aware resolution: shaped locals (`f:read` after
    // `local f = io.open(...)`) resolve through their shape, and a
    // shadowed base (`local string = 1; string.format`) refuses a
    // stdlib hover instead of showing the wrong docs.
    let entry = match &target {
        CursorTarget::Identifier { name, span } => {
            if doc.semantic.resolves_to_local(name, *span) {
                return None;
            }
            library_for(doc.target.lua_version(), doc.target.stdlib_environment())
                .lookup_str(&path)?
        }
        CursorTarget::DottedPath { spans, .. } => {
            doc.semantic.resolve_stdlib_path(&path, *spans.first()?)?
        }
        CursorTarget::Call { base_span, .. } => {
            doc.semantic.resolve_stdlib_path(&path, (*base_span)?)?
        }
    };
    let range = Some(span_to_range(doc, &target));
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: render_entry(&path, entry, doc.target.lua_version()),
        }),
        range,
    })
}

fn span_to_range(doc: &DocumentState, target: &CursorTarget) -> Range {
    let span = target.span();
    doc.line_index.range(&doc.text, span.start, span.end)
}

fn render_entry(path: &[&str], entry: &StdlibEntry, version: LuaVersion) -> String {
    let mut out = String::new();
    // Colon-methods render with the call syntax that reaches them.
    let full_path = match entry {
        StdlibEntry::Function(func) if func.is_method && path.len() >= 2 => {
            format!(
                "{}:{}",
                path[..path.len() - 1].join("."),
                path[path.len() - 1]
            )
        }
        _ => path.join("."),
    };
    match entry {
        StdlibEntry::Function(func) => {
            out.push_str("```lua\n");
            for (idx, sig) in func.signatures.iter().enumerate() {
                if idx > 0 {
                    out.push('\n');
                }
                out.push_str(&render_signature(&full_path, sig.params.as_slice()));
            }
            out.push_str("\n```\n");
            if let Some(dep) = &func.deprecated {
                out.push_str(&format!("\n**Deprecated:** {}\n", dep.message));
            }
            let mut tags: Vec<&str> = Vec::new();
            if func.must_use {
                tags.push("must_use");
            }
            if func.is_pure {
                tags.push("pure");
            }
            if !func.read_only {
                tags.push("rebindable");
            }
            if !tags.is_empty() {
                out.push_str(&format!("\n_{}_\n", tags.join(" · ")));
            }
        }
        StdlibEntry::Namespace(namespace) => {
            out.push_str(&format!(
                "**`{full_path}`** namespace · {} entries\n",
                namespace.members.len()
            ));
            if let Some(dep) = &namespace.deprecated {
                out.push_str(&format!("\n**Deprecated:** {}\n", dep.message));
            }
        }
        StdlibEntry::Constant(value) => {
            out.push_str(&format!("```lua\n{full_path}  -- constant\n```\n"));
            if let Some(dep) = &value.deprecated {
                out.push_str(&format!("\n**Deprecated:** {}\n", dep.message));
            }
        }
        StdlibEntry::Property(value) => {
            let rw = if value.read_only {
                "read-only"
            } else {
                "read/write"
            };
            out.push_str(&format!("```lua\n{full_path}  -- {rw} property\n```\n"));
            if let Some(dep) = &value.deprecated {
                out.push_str(&format!("\n**Deprecated:** {}\n", dep.message));
            }
        }
    }
    out.push_str(&format!("\n_{}_", version_label(version)));
    out
}

fn render_signature(name: &str, params: &[StdlibParam]) -> String {
    let rendered = params
        .iter()
        .map(render_param)
        .collect::<Vec<_>>()
        .join(", ");
    format!("function {name}({rendered})")
}

fn render_param(param: &StdlibParam) -> String {
    if matches!(param.kind, StdlibArgKind::Vararg) {
        return "...".to_string();
    }
    let mut out = param_label(param);
    if let StdlibArgKind::Constant(values) = &param.kind {
        // Large generated sets (Roblox service and class names) would
        // swamp the panel; show a prefix and the total.
        let allowed = values
            .iter()
            .take(CONSTANT_PREVIEW_LIMIT)
            .map(|constant| format!("\"{}\"", constant.value))
            .collect::<Vec<_>>()
            .join(" | ");
        if values.len() > CONSTANT_PREVIEW_LIMIT {
            out.push_str(&format!(": {allowed} | ... ({} values)", values.len()));
        } else {
            out.push_str(&format!(": {allowed}"));
        }
    }
    out
}

const CONSTANT_PREVIEW_LIMIT: usize = 8;

fn version_label(version: LuaVersion) -> &'static str {
    match version {
        LuaVersion::Lua51 => "Lua 5.1",
        LuaVersion::Lua52 => "Lua 5.2",
        LuaVersion::Lua53 => "Lua 5.3",
        LuaVersion::Lua54 => "Lua 5.4",
        LuaVersion::Lua55 => "Lua 5.5",
        LuaVersion::Luau => "Luau",
    }
}
