//! The ignored `regenerate_luau_definitions` test, which writes `luau.d.luau`
//! from Luau's built-in definitions and Luau's library reference, and offline
//! checks of the committed file.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use luck_allocator::Allocator;
use luck_ast::{Declaration, GetSpan as _, Statement, Type, TypeField, is_valid_identifier};
use luck_type_checker::{Checker, builtin_definitions};

use crate::common::NoModules;

const REFERENCE_URL: &str =
    "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/reference/library.md";

const HEADER: &str = "\
-- Luau's standard library as a Luau definition file, which `just stdlib`
-- generates from two sources: Luau's built-in definitions, the text
-- luck_type_checker's builtin_definitions gives from
-- https://github.com/luau-lang/luau, and Luau's library reference,
-- src/content/docs/reference/library.md in https://github.com/luau-lang/site.
--
-- The declarations are the built-in definitions, and a declaration from the
-- reference's signature for each function it documents that they leave to
-- Luau's C++, such as the string library. The comment above a declaration is
-- the reference's description of it, and a function the reference calls
-- deprecated is `@[deprecated]` with its advice as the reason. Both sources
-- are under the MIT license, (c) Roblox Corporation and Lua.org, PUC-Rio,
-- whose text is in LICENSE-luau beside this file.

";

fn generated_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib_data/luau.d.luau")
}

/// A name the reference documents, as `string.byte`, with each signature its
/// code block gives it, the part after the name, and the paragraph after the
/// block.
struct Documented {
    name: String,
    signatures: Vec<String>,
    paragraph: String,
}

/// Every name the reference documents, in its order.
fn documented(reference: &str) -> Vec<Documented> {
    let mut items: Vec<Documented> = Vec::new();
    let mut lines = reference.lines().map(str::trim_end).peekable();
    while let Some(fence) = lines.next() {
        if fence != "```" {
            continue;
        }
        let block: Vec<&str> = lines.by_ref().take_while(|code| *code != "```").collect();
        while lines.next_if(|blank| blank.is_empty()).is_some() {}
        let mut paragraph: Vec<&str> = Vec::new();
        while let Some(prose) = lines.next_if(|text| !text.is_empty() && !text.starts_with("```")) {
            paragraph.push(prose.trim());
        }
        let paragraph = paragraph.join(" ");
        let first = items.len();
        for code in block {
            let signature = code.strip_prefix("function ").unwrap_or(code).trim();
            let name_length = signature.find(['(', '<']).unwrap_or(signature.len());
            let (name, rest) = signature.split_at(name_length);
            match items[first..].iter_mut().find(|item| item.name == name) {
                Some(item) => item.signatures.push(rest.to_owned()),
                None => items.push(Documented {
                    name: name.to_owned(),
                    signatures: vec![rest.to_owned()],
                    paragraph: paragraph.clone(),
                }),
            }
        }
    }
    items
}

/// `text` split at its commas outside brackets, each part trimmed.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0);
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        match character {
            '-' if characters.next_if(|(_, next)| *next == '>').is_some() => {}
            '(' | '{' | '[' | '<' => depth += 1,
            ')' | '}' | ']' | '>' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(text[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts.retain(|part| !part.is_empty());
    parts
}

/// The index just past the bracket that closes the one `text` opens with.
fn closing(text: &str) -> usize {
    let mut depth = 0usize;
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        match character {
            '-' if characters.next_if(|(_, next)| *next == '>').is_some() => {}
            '(' | '{' | '[' | '<' => depth += 1,
            ')' | '}' | ']' | '>' => {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
    }
    panic!("`{text}` doesn't close its bracket");
}

/// A type the reference writes, with each of the pseudo-types it writes for
/// documentation as the Luau type Luau's checker binds for it: `table` as
/// `{}`, `function` as any function, and `userdata` as `any`.
fn luau_type(documented: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    for character in documented.chars().chain([' ']) {
        if character.is_ascii_alphanumeric() || character == '_' {
            word.push(character);
            continue;
        }
        out.push_str(match word.as_str() {
            "table" => "{}",
            "function" => "((...any) -> ...any)",
            "userdata" => "any",
            other => other,
        });
        word.clear();
        out.push(character);
    }
    out.pop();
    out
}

fn optional(ty: &str) -> String {
    if ty.ends_with('?') {
        ty.to_owned()
    } else if ty.contains(" | ") || ty.contains("->") {
        format!("({ty})?")
    } else {
        format!("{ty}?")
    }
}

/// What a function the reference documents returns, as Luau writes it:
/// `<iterator>`, the values a `for` loop iterates with, as any values, and
/// `(A, B)?`, the values or none, as each of them optional.
fn luau_returns(documented: &str) -> String {
    if documented == "<iterator>" {
        return "...any".to_owned();
    }
    if let Some(values) = documented.strip_prefix('(').and_then(|rest| rest.strip_suffix(")?")) {
        let optionals: Vec<String> =
            split_top_level(values).into_iter().map(|value| optional(&luau_type(value))).collect();
        return format!("({})", optionals.join(", "));
    }
    luau_type(documented)
}

/// A function the reference documents as a declaration's text, from its one
/// signature: `(params): returns` for a `declare function`, or
/// `(params) -> returns` for a table field's function type, each after its
/// generics. A parameter the reference writes `name: ...T`, the rest of the
/// arguments, is a vararg.
fn declaration(item: &Documented, is_type: bool) -> String {
    let [signature] = item.signatures.as_slice() else {
        panic!("{} has overloads, which only a table field could declare", item.name);
    };
    let generics_length = if signature.starts_with('<') { closing(signature) } else { 0 };
    let (generics, rest) = signature.split_at(generics_length);
    let params_length = closing(rest);
    let (params, returned) = rest.split_at(params_length);
    let returned = returned.trim().strip_prefix(':').map(str::trim).filter(|ty| !ty.is_empty());
    let params: Vec<String> = split_top_level(&params[1..params.len() - 1])
        .into_iter()
        .map(|param| {
            let (name, ty) = param.split_once(':').expect("a documented parameter has a type");
            let ty = ty.trim();
            match ty.strip_prefix("...") {
                Some(each) if is_type => format!("...{}", luau_type(each)),
                Some(each) => format!("...: {}", luau_type(each)),
                None => format!("{}: {}", name.trim(), luau_type(ty)),
            }
        })
        .collect();
    let returned = returned.map(luau_returns);
    let returns = match (returned, is_type) {
        (Some(returned), true) => format!(" -> {returned}"),
        (None, true) => " -> ()".to_owned(),
        (Some(returned), false) => format!(": {returned}"),
        (None, false) => String::new(),
    };
    format!("{generics}({}){returns}", params.join(", "))
}

/// The attribute a function the paragraph calls deprecated takes, with the
/// advice after the `;` of the sentence that says so as its reason, and its
/// code as what to use when that's a function's name, and `None` for a
/// function it doesn't call deprecated.
fn deprecated_attribute(paragraph: &str) -> Option<String> {
    let sentence = paragraph.split(". ").find(|sentence| sentence.contains("deprecated"))?;
    let Some((_, advice)) = sentence.split_once("; ") else {
        return Some("@deprecated ".to_owned());
    };
    let advice = advice.trim_end_matches('.');
    let replacement = advice
        .split('`')
        .nth(1)
        .filter(|code| code.split('.').all(is_valid_identifier))
        .map(|code| format!("use = \"{code}\", "))
        .unwrap_or_default();
    Some(format!("@[deprecated {{{replacement}reason = \"{}\"}}] ", advice.replace('"', "\\\"")))
}

/// A declaration of the built-in definitions: where its line starts, and
/// where an attribute goes in it.
struct BuiltIn {
    name: String,
    line_start: usize,
    attribute_at: usize,
}

/// The built-in definitions' declarations, and each table type a global has,
/// by the global's name, with where the line that closes it starts.
fn built_ins(source: &str) -> (Vec<BuiltIn>, Vec<(String, usize)>) {
    let line_start = |offset: u32| source[..offset as usize].rfind('\n').map_or(0, |end| end + 1);
    let allocator = Allocator::new();
    let parsed = luck_parser::parse_definitions(&allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let mut declared = Vec::new();
    let mut tables = Vec::new();
    for statement in &parsed.chunk.stmts {
        let Statement::Declaration(declaration) = statement else { continue };
        if let Declaration::Function(function) = &**declaration {
            declared.push(BuiltIn {
                name: function.name.text.as_str().to_owned(),
                line_start: line_start(function.span.start),
                attribute_at: line_start(function.span.start),
            });
        } else if let Declaration::Global(global) = &**declaration
            && let Type::Table(table) = &global.ty
        {
            let library = global.name.text.as_str();
            for field in &table.fields {
                let TypeField::Named { name, value, span, .. } = field else { continue };
                declared.push(BuiltIn {
                    name: format!("{library}.{}", name.text.as_str()),
                    line_start: line_start(span.start),
                    attribute_at: value.span().start as usize,
                });
            }
            tables.push((library.to_owned(), line_start(table.span.end - 1)));
        }
    }
    (declared, tables)
}

/// `built_in` with the reference's descriptions and deprecations, and the
/// functions it documents that `built_in` doesn't declare.
fn generate(built_in: &str, reference: &str) -> String {
    let (declared, tables) = built_ins(built_in);
    let items = documented(reference);
    let mut edits: Vec<(usize, String)> = Vec::new();
    let mut new_libraries: Vec<(&str, String)> = Vec::new();
    let mut appended = String::new();
    for item in &items {
        let comment = format!("-- {}\n", item.paragraph);
        let deprecated = deprecated_attribute(&item.paragraph).unwrap_or_default();
        if let Some(found) = declared.iter().find(|found| found.name == item.name) {
            let line = &built_in[found.line_start..];
            let indent = &line[..line.len() - line.trim_start().len()];
            edits.push((found.line_start, format!("{indent}{comment}")));
            if !deprecated.is_empty() {
                edits.push((found.attribute_at, deprecated));
            }
            continue;
        }
        let Some((library, member)) = item.name.split_once('.') else {
            write!(
                appended,
                "\n{comment}{deprecated}declare function {}{}\n",
                item.name,
                declaration(item, false)
            )
            .expect("write to a string");
            continue;
        };
        let field =
            format!("    {comment}    {member}: {deprecated}{},\n", declaration(item, true));
        match tables.iter().find(|(name, _)| name == library) {
            Some((_, closing_line)) => edits.push((*closing_line, field)),
            None => match new_libraries.iter_mut().find(|(name, _)| *name == library) {
                Some((_, fields)) => fields.push_str(&field),
                None => new_libraries.push((library, field)),
            },
        }
    }
    for (library, fields) in new_libraries {
        write!(appended, "\ndeclare {library}: {{\n{fields}}}\n").expect("write to a string");
    }
    edits.sort_by_key(|(offset, _)| *offset);
    let mut out = String::from(HEADER);
    let mut copied = 0;
    for (offset, text) in edits {
        out.push_str(&built_in[copied..offset]);
        out.push_str(&text);
        copied = offset;
    }
    out.push_str(&built_in[copied..]);
    out.push_str(&appended);
    let trimmed = out.trim_end().to_owned() + "\n";
    assert!(trimmed.is_ascii(), "the definitions aren't ASCII");
    trimmed
}

#[test]
#[ignore = "rewrites luau.d.luau from Luau's built-in definitions and its live library reference; run with --ignored"]
fn regenerate_luau_definitions() {
    let reference = ureq::get(REFERENCE_URL)
        .call()
        .expect("fetch Luau's library reference")
        .body_mut()
        .read_to_string()
        .expect("read Luau's library reference");
    let text = generate(&builtin_definitions(), &reference.replace("\r\n", "\n"));
    fs::write(generated_path(), text).expect("write luau.d.luau");
}

fn committed() -> String {
    fs::read_to_string(generated_path()).expect("read luau.d.luau")
}

/// Its lines hold the built-in definitions' in order, once the deprecations
/// the reference adds are gone, so it's regenerated when `just luau` moves
/// Luau.
#[test]
fn the_definitions_hold_luaus_built_in_ones() {
    let committed = committed();
    let mut lines = committed.lines().map(|line| match line.split_once("@[deprecated {") {
        Some((before, after)) => {
            let (_, rest) = after.split_once("}] ").expect("an attribute ends");
            format!("{before}{rest}")
        }
        None => line.to_owned(),
    });
    for built_in in builtin_definitions().lines().filter(|line| !line.trim().is_empty()) {
        assert!(
            lines.any(|line| line == built_in),
            "luau.d.luau lacks `{built_in}`; regenerate it with `just stdlib`"
        );
    }
}

#[test]
fn the_definitions_parse() {
    let committed = committed();
    let allocator = Allocator::new();
    let parsed = luck_parser::parse_definitions(&allocator, committed.as_bytes());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
}

#[test]
fn the_definitions_load_into_a_checker() {
    let mut checker = Checker::new(NoModules);
    let loaded = checker.load_definitions("@luau", &committed());
    assert!(loaded.is_ok(), "{loaded:?}");
}
