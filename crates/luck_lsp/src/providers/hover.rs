//! Hover provider. Looks up the identifier under the cursor in the rich
//! stdlib model and renders signature, deprecation, must-use, and Roblox
//! markers as a markdown panel.

use luck_semantic::stdlib_model::{
    EntryKind, StdlibArgKind, StdlibEntry, StdlibFunction, StdlibParam, library_for,
};
use luck_token::LuaVersion;
use tower_lsp::lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind, Range};

use crate::backend::DocumentState;
use crate::providers::cursor::{CursorTarget, find_target_at};

#[must_use]
pub fn hover(doc: &DocumentState, params: &HoverParams) -> Option<Hover> {
    let position = params.text_document_position_params.position;
    let offset = doc.line_index.offset(&doc.text, position);
    let target = find_target_at(&doc.parsed.block, offset)?;
    let path = target.path();
    let environment = doc.target.stdlib_environment();
    let entry = library_for(doc.target.lua_version())
        .lookup_str(&path)
        .filter(|entry| entry.available_in_luau(environment))?;
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
    let full_path = path.join(".");
    match &entry.kind {
        EntryKind::Function(func) => {
            out.push_str("```lua\n");
            out.push_str(&render_signature(&full_path, func));
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
            if matches!(
                entry.luau_tier,
                luck_semantic::stdlib_model::LuauTier::Roblox
            ) {
                tags.push("Roblox-only");
            }
            if !tags.is_empty() {
                out.push_str(&format!("\n_{}_\n", tags.join(" · ")));
            }
        }
        EntryKind::Namespace(members) => {
            out.push_str(&format!(
                "**`{full_path}`** namespace · {} entries\n",
                members.len()
            ));
            if matches!(
                entry.luau_tier,
                luck_semantic::stdlib_model::LuauTier::Roblox
            ) {
                out.push_str("\n_Roblox-only_\n");
            }
        }
        EntryKind::Constant(value) => {
            out.push_str(&format!("```lua\n{full_path}  -- constant\n```\n"));
            if let Some(dep) = &value.deprecated {
                out.push_str(&format!("\n**Deprecated:** {}\n", dep.message));
            }
        }
        EntryKind::Property(value) => {
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

fn render_signature(name: &str, func: &StdlibFunction) -> String {
    let params = func
        .params
        .iter()
        .map(render_param)
        .collect::<Vec<_>>()
        .join(", ");
    format!("function {name}({params})")
}

fn render_param(param: &StdlibParam) -> String {
    let mut out = String::new();
    let mut name = match &param.kind {
        StdlibArgKind::Any => "any",
        StdlibArgKind::Bool => "bool",
        StdlibArgKind::Number => "number",
        StdlibArgKind::String => "string",
        StdlibArgKind::Function => "function",
        StdlibArgKind::Table => "table",
        StdlibArgKind::Nil => "nil",
        StdlibArgKind::Display(d) => d.as_str(),
        StdlibArgKind::Constant(_) => "constant",
        StdlibArgKind::Vararg => "...",
    }
    .to_string();
    if matches!(param.kind, StdlibArgKind::Vararg) {
        return "...".to_string();
    }
    if !param.required {
        name = format!("{name}?");
    }
    out.push_str(&name);
    if let StdlibArgKind::Constant(values) = &param.kind {
        let allowed = values
            .iter()
            .map(|v| format!("\"{v}\""))
            .collect::<Vec<_>>()
            .join(" | ");
        out.push_str(&format!(": {allowed}"));
    }
    out
}

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
