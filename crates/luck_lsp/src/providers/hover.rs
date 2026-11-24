use core::ptr;

use ls_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};
use luck_ast::{AstKind, GetSpan as _, Suffix};
use luck_stdlib::{StdlibArgKind, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibParam};
use luck_token::LuaVersion;

use crate::document::DocumentState;
use crate::providers::cursor::{TextKind, name_at, text_kind_at};
use crate::providers::stdlib_render::{constant_preview, param_label};

pub(crate) fn hover(document: &DocumentState, position: Position) -> Option<Hover> {
    let offset = document.offset(position);
    if text_kind_at(document, offset) != TextKind::Code {
        return None;
    }
    let (id, name) = name_at(document, offset)?;
    let semantic = document.semantic();
    let tree = semantic.scoping();
    let nodes = document.nodes();
    // The path the name ends, root first: the name alone where it's read or
    // written, the fields up to it where it's a field, and its receiver's
    // path where it names a method.
    let (path, root, span): (Vec<&str>, _, _) = if tree.reference_of(name).is_some() {
        (vec![name.text.as_str()], name, name.span)
    } else {
        let parent = nodes.parent_kind(id)?;
        let (head, method) = if let AstKind::SuffixedExpression(chain) = parent {
            let field = chain.suffixes.iter().position(
                |suffix| matches!(suffix, Suffix::Field(field) if ptr::eq(field, name)),
            )?;
            (chain.head(field.saturating_add(1)), None)
        } else if let AstKind::MethodCall(method_call) = parent {
            let chain = nodes.parent_kind(nodes.parent_id(id)?)?.as_suffixed_expr()?;
            let call = chain.suffixes.iter().position(|suffix| {
                matches!(suffix, Suffix::MethodCall(call) if ptr::eq(call.as_ref(), method_call))
            })?;
            (chain.head(call), Some(name))
        } else {
            return None;
        };
        let dotted = head.dotted_path()?;
        let segments = dotted.segments().chain(method.map(|called| called.text.as_str()));
        let span = method.map_or_else(|| head.span(), |called| head.span().merge(called.span));
        (segments.collect(), dotted.root(), span)
    };
    // Resolving from the root's reference lets shaped locals (`f:read` after
    // `local f = io.open(...)`) resolve through their shape, and gives a
    // shadowed base (`local string = 1; string.format`) no stdlib hover
    // instead of the wrong docs.
    let entry = semantic.resolve_stdlib_path(&path, tree.reference(tree.reference_of(root)?))?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: render_entry(&path, entry, document.version()),
        }),
        range: Some(document.range(span)),
    })
}

fn render_entry(path: &[&str], entry: &StdlibEntry<'_>, version: LuaVersion) -> String {
    let full_path = render_path(path, entry);
    let body = match entry {
        StdlibEntry::Function(function) => render_function(&full_path, function),
        StdlibEntry::Namespace(namespace) => format!(
            "**`{full_path}`** namespace \u{b7} {} entries\n{}",
            namespace.members.len(),
            deprecation_note(namespace.deprecated.as_ref()),
        ),
        StdlibEntry::Constant(value) => format!(
            "```lua\n{full_path}  -- constant\n```\n{}",
            deprecation_note(value.deprecated.as_ref()),
        ),
        StdlibEntry::Property(value) => {
            let access = if value.is_read_only { "read-only" } else { "read/write" };
            format!(
                "```lua\n{full_path}  -- {access} property\n```\n{}",
                deprecation_note(value.deprecated.as_ref()),
            )
        }
    };
    format!("{body}\n_{}_", version_label(version))
}

/// Methods render with colon syntax (`game:GetService`).
fn render_path(path: &[&str], entry: &StdlibEntry<'_>) -> String {
    if let StdlibEntry::Function(function) = entry
        && function.is_method
        && let Some((method, receiver)) = path.split_last()
        && !receiver.is_empty()
    {
        format!("{}:{method}", receiver.join("."))
    } else {
        path.join(".")
    }
}

fn render_function(full_path: &str, function: &StdlibFunction<'_>) -> String {
    let signatures = function
        .signatures
        .iter()
        .map(|signature| render_signature(full_path, signature.params))
        .collect::<Vec<_>>()
        .join("\n");
    let mut tags: Vec<&str> = Vec::new();
    if function.is_must_use {
        tags.push("must_use");
    }
    if function.is_pure {
        tags.push("pure");
    }
    if !function.is_read_only {
        tags.push("rebindable");
    }
    let tag_line =
        if tags.is_empty() { String::new() } else { format!("\n_{}_\n", tags.join(" \u{b7} ")) };
    format!(
        "```lua\n{signatures}\n```\n{}{tag_line}",
        deprecation_note(function.deprecated.as_ref())
    )
}

fn deprecation_note(deprecated: Option<&StdlibDeprecation<'_>>) -> String {
    deprecated.map_or_else(String::new, |deprecation| {
        format!("\n**Deprecated:** {}\n", deprecation.message)
    })
}

fn render_signature(name: &str, params: &[StdlibParam<'_>]) -> String {
    let rendered = params.iter().map(render_param).collect::<Vec<_>>().join(", ");
    format!("function {name}({rendered})")
}

fn render_param(param: &StdlibParam<'_>) -> String {
    let label = param_label(param);
    let StdlibArgKind::Constant(values) = &param.kind else {
        return label;
    };
    let allowed = constant_preview(values, " | ", |constant| format!("\"{}\"", constant.value));
    format!("{label}: {allowed}")
}

const fn version_label(version: LuaVersion) -> &'static str {
    match version {
        LuaVersion::Lua51 => "Lua 5.1",
        LuaVersion::Lua52 => "Lua 5.2",
        LuaVersion::Lua53 => "Lua 5.3",
        LuaVersion::Lua54 => "Lua 5.4",
        LuaVersion::Lua55 => "Lua 5.5",
        LuaVersion::Luau => "Luau",
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{LuaTarget, index_to_offset};

    use super::*;
    use crate::fixtures::document;

    fn markdown_at(document: &DocumentState, line: u32, utf16_column: u32) -> Option<String> {
        let HoverContents::Markup(markup) =
            hover(document, Position::new(line, utf16_column))?.contents
        else {
            panic!("hover should return markup contents");
        };
        Some(markup.value)
    }

    #[test]
    fn hover_on_enum_item_renders_constant() {
        let text = "local m = Enum.Material.Grass";
        let document = document(text, LuaTarget::LuauRoblox);
        // Cursor on `Grass`.
        let utf16_column = index_to_offset(text.find("Grass").expect("item") + 2);
        let markdown = markdown_at(&document, 0, utf16_column).expect("hover on enum item");
        assert!(markdown.contains("Enum.Material.Grass"), "hover names the full path: {markdown}");
    }

    #[test]
    fn hover_on_shaped_local_method_renders_colon_form() {
        let text = "\
local f = io.open('x')
local n = f:seek('set')";
        let document = document(text, LuaTarget::Lua54);
        // Cursor on `seek`.
        let markdown = markdown_at(&document, 1, 14).expect("hover on shaped local method");
        assert!(markdown.contains("f:seek("), "colon rendering expected: {markdown}");
    }

    #[test]
    fn hover_refuses_shadowed_namespace_member() {
        let text = "\
local string = 1
local x = string.format";
        let document = document(text, LuaTarget::Lua54);
        // Cursor on `format`.
        assert!(
            markdown_at(&document, 1, 19).is_none(),
            "shadowed base must not hover stdlib docs"
        );
    }

    #[test]
    fn hover_covers_the_path_a_member_ends() {
        let document = document("local s = string.format", LuaTarget::Lua54);
        let hovered = hover(&document, Position::new(0, 19)).expect("hover on `format`");
        let range = hovered.range.expect("a range");
        assert_eq!((range.start.character, range.end.character), (10, 23), "{range:?}");
    }

    #[test]
    fn hover_reads_only_a_name_that_reaches_the_global() {
        let document = document("local tostring = tostring", LuaTarget::Lua54);
        assert!(markdown_at(&document, 0, 8).is_none(), "the declaration names a local");
        let read = markdown_at(&document, 0, 20).expect("the initializer reads the global");
        assert!(read.contains("function tostring("), "{read}");
    }

    #[test]
    fn hover_on_game_get_service_uses_colon_syntax() {
        let text = "local p = game:GetService('RunService')";
        let document = document(text, LuaTarget::LuauRoblox);
        let utf16_column = index_to_offset(text.find("GetService").expect("method") + 4);
        let markdown = markdown_at(&document, 0, utf16_column).expect("hover on GetService");
        assert!(markdown.contains("game:GetService("), "colon rendering expected: {markdown}");
    }

    #[test]
    fn hover_ignores_a_field_of_a_call_result() {
        let text = "local printer = make().print\n";
        let document = document(text, LuaTarget::Lua54);
        let utf16_column = index_to_offset(text.find(".print").expect("field") + 3);
        let markdown = markdown_at(&document, 0, utf16_column);
        assert!(
            markdown.is_none(),
            "a field of a call result isn't the global `print`: {markdown:?}"
        );
    }

    #[test]
    fn hover_on_a_statement_level_method_call_resolves() {
        let text = "\
local f = io.open('x')
f:seek('set')
";
        let document = document(text, LuaTarget::Lua54);
        let markdown = markdown_at(&document, 1, 3);
        assert!(
            markdown.as_deref().is_some_and(|rendered| rendered.contains("f:seek(")),
            "a method call in statement position hovers like one in an expression: {markdown:?}"
        );
    }

    #[test]
    fn hover_ignores_a_comment_inside_a_call() {
        let text = "print(--[[ print ]] 1)\n";
        let document = document(text, LuaTarget::Lua54);
        assert!(markdown_at(&document, 0, 13).is_none());
    }
}
