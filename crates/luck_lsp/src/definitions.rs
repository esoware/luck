//! The definition files a checker loads, whose declarations hover,
//! completion, and signature help document, and go to definition goes to.
//!
//! A declaration is found by the documentation symbol Luau gives what it
//! declares, as `@roblox/globaltype/Part.Anchored`, which names the
//! definition file it was loaded from and the declaration in it, or else by
//! where Analysis records it, which it does for a member a symbol doesn't
//! reach, such as a field of a field of a declared global. Its documentation
//! is the comment directly above it. Luau's own definitions and Roblox's are
//! in no workspace, so the server serves them under its own `luck:` scheme,
//! and every checker shares one package of each.

use std::borrow::Cow;
use std::str::FromStr as _;
use std::sync::{Arc, OnceLock};

use ls_types::{Location, Uri};
use luck_allocator::Allocator;
use luck_ast::{ExternTypeMember, Statement, Type, TypeDeclarationValue, TypeField};
use luck_stdlib::{SecurityLevel, luau_definitions, roblox_definitions};
use luck_token::{Comment, CommentPosition, Span, index_to_offset, offset_to_index};
use luck_type_checker::Declaration;
use rustc_hash::FxHashMap;

use crate::document::span_text;
use crate::line_index::{LineIndex, PositionEncoding};

/// The scheme of the definition files the server serves itself, through
/// `workspace/textDocumentContent`.
pub(crate) const SCHEME: &str = "luck";

/// Where the server serves Luau's own definitions, which document its
/// built-in library.
fn luau_uri() -> Uri {
    Uri::from_str("luck:/luau.d.luau").expect("a valid URI")
}

/// Where the server serves Roblox's API at `security`.
fn roblox_uri(security: SecurityLevel) -> Uri {
    Uri::from_str(&format!("luck:/roblox/{}.d.luau", security.name())).expect("a valid URI")
}

/// Luau's own definitions, which every checker starts with.
pub(crate) fn luau_package() -> Arc<Package> {
    static PACKAGE: OnceLock<Arc<Package>> = OnceLock::new();
    let package = PACKAGE.get_or_init(|| {
        let text = Cow::Borrowed(luau_definitions());
        Arc::new(Package::new("@luau".to_owned(), Some(luau_uri()), text))
    });
    Arc::clone(package)
}

/// Roblox's API at `security`, which `@roblox` loads.
pub(crate) fn roblox_package(security: SecurityLevel) -> Arc<Package> {
    static PACKAGES: [OnceLock<Arc<Package>>; SecurityLevel::ALL.len()] =
        [const { OnceLock::new() }; SecurityLevel::ALL.len()];
    let package = PACKAGES[security as usize].get_or_init(|| {
        let text = Cow::Borrowed(roblox_definitions(security));
        Arc::new(Package::new("@roblox".to_owned(), Some(roblox_uri(security)), text))
    });
    Arc::clone(package)
}

/// The text of the definition file the server serves at `uri`.
pub(crate) fn served_text(uri: &Uri) -> Option<&'static str> {
    if *uri == luau_uri() {
        return Some(luau_definitions());
    }
    let level = SecurityLevel::ALL.into_iter().find(|level| roblox_uri(*level) == *uri)?;
    Some(roblox_definitions(level))
}

/// A definition file a checker loads.
#[derive(Debug)]
pub(crate) struct Package {
    /// What the checker loads it under, which the documentation symbols of
    /// what it declares start with.
    name: String,
    /// `None` for a file no URI names.
    uri: Option<Uri>,
    text: Cow<'static, str>,
    declarations: OnceLock<Declarations>,
}

/// What a request reads of a definition file, parsed the first time one
/// does.
#[derive(Debug)]
struct Declarations {
    /// Where each declaration starts, which its documentation is above, and
    /// its name, by its documentation symbol without the file's name.
    by_symbol: FxHashMap<String, (u32, Span)>,
    comments: Vec<Comment>,
}

/// A declaration in a definition file.
pub(crate) struct Declared<'package> {
    package: &'package Package,
    declarations: &'package Declarations,
    start: u32,
    name: Span,
}

impl Package {
    pub(crate) const fn new(name: String, uri: Option<Uri>, text: Cow<'static, str>) -> Self {
        Self { name, uri, text, declarations: OnceLock::new() }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) const fn uri(&self) -> Option<&Uri> {
        self.uri.as_ref()
    }

    /// The declaration `declaration` refers to in one of `packages`.
    pub(crate) fn find<'package>(
        packages: &'package [Arc<Self>],
        declaration: &Declaration,
    ) -> Option<Declared<'package>> {
        let by_symbol = declaration.documentation_symbol.as_deref().and_then(|symbol| {
            // An overload's symbol names the function it overloads first.
            let symbol = symbol.split_once("/overload/").map_or(symbol, |(function, _)| function);
            let (package, declared) = packages
                .iter()
                .filter_map(|package| {
                    Some((package, symbol.strip_prefix(&package.name)?.strip_prefix('/')?))
                })
                .max_by_key(|(package, _)| package.name.len())?;
            let declarations = package.declarations();
            let &(start, name) = declarations.by_symbol.get(declared)?;
            Some(Declared { package, declarations, start, name })
        });
        by_symbol.or_else(|| {
            let (module, span) = declaration.definition.as_ref()?;
            let package = packages.iter().find(|package| package.name == *module)?;
            let declarations = package.declarations();
            Some(Declared { package, declarations, start: span.start, name: *span })
        })
    }

    fn declarations(&self) -> &Declarations {
        self.declarations.get_or_init(|| {
            let allocator = Allocator::new();
            let parsed = luck_parser::parse_definitions(&allocator, self.text.as_bytes());
            let mut by_symbol = FxHashMap::default();
            let mut declare = |symbol: String, start: u32, name: Span| {
                by_symbol.entry(symbol).or_insert((start, name));
            };
            for statement in &parsed.chunk.stmts {
                #[expect(
                    clippy::wildcard_enum_match_arm,
                    reason = "only these statements declare, and a definition file holds no other"
                )]
                match statement {
                    Statement::Declaration(declaration) => match &**declaration {
                        luck_ast::Declaration::Global(global) => {
                            let name = global.name.text.as_str();
                            declare(format!("global/{name}"), global.span.start, global.name.span);
                            fields(&global.ty, &mut |field, start, span| {
                                declare(format!("global/{name}.{field}"), start, span);
                            });
                        }
                        luck_ast::Declaration::Function(function) => {
                            let name = function.name.text.as_str();
                            let start = function.span.start;
                            declare(format!("global/{name}"), start, function.name.span);
                        }
                        luck_ast::Declaration::ExternType(extern_type) => {
                            let type_name = extern_type.name.text.as_str();
                            declare(
                                format!("globaltype/{type_name}"),
                                extern_type.span.start,
                                extern_type.name.span,
                            );
                            for member in &extern_type.members {
                                let (start, member_name) = match member {
                                    ExternTypeMember::Field(TypeField::Named {
                                        span,
                                        name,
                                        ..
                                    }) => (span.start, name),
                                    ExternTypeMember::Method(method) => {
                                        (method.span.start, &method.name)
                                    }
                                    ExternTypeMember::Field(
                                        TypeField::Indexer { .. } | TypeField::Array { .. },
                                    ) => continue,
                                };
                                let symbol = format!("globaltype/{type_name}.{}", member_name.text);
                                declare(symbol, start, member_name.span);
                            }
                        }
                    },
                    Statement::TypeDeclaration(alias) => {
                        let name = alias.name.text.as_str();
                        declare(format!("globaltype/{name}"), alias.span.start, alias.name.span);
                        if let TypeDeclarationValue::Alias(ty) = &alias.value {
                            fields(ty, &mut |field, start, span| {
                                declare(format!("globaltype/{name}.{field}"), start, span);
                            });
                        }
                    }
                    _ => {}
                }
            }
            Declarations { by_symbol, comments: parsed.comments }
        })
    }
}

/// Hands `visit` each named field of a table type, or of the tables an
/// intersection takes its parts from, with where it starts and its name.
fn fields(ty: &Type<'_>, visit: &mut dyn FnMut(&str, u32, Span)) {
    #[expect(clippy::wildcard_enum_match_arm, reason = "only a table type has fields")]
    match ty {
        Type::Table(table) => {
            for field in &table.fields {
                if let TypeField::Named { span, name, .. } = field {
                    visit(name.text.as_str(), span.start, name.span);
                }
            }
        }
        Type::Intersection(intersection) => {
            for part in &intersection.types {
                fields(part, visit);
            }
        }
        _ => {}
    }
}

impl Declared<'_> {
    /// The comments directly above the declaration, each on its own line,
    /// without their delimiters.
    pub(crate) fn documentation(&self) -> Option<String> {
        let text = self.package.text();
        let comments = &self.declarations.comments;
        let mut above = line_start(text, self.start);
        // A member on the line of what holds it has no line of its own above.
        if !span_text(text, Span::new(above, self.start)).trim().is_empty() {
            return None;
        }
        let mut lines = Vec::new();
        let before = comments.partition_point(|comment| comment.span.end <= above);
        for comment in comments[..before].iter().rev() {
            let gap = span_text(text, Span::new(comment.span.end, above));
            let is_directly_above = comment.position == CommentPosition::Leading
                && gap.trim_matches([' ', '\t', '\r']) == "\n";
            if !is_directly_above {
                break;
            }
            let written = span_text(text, comment.span).as_bytes();
            let Some(body) = comment.kind.body(written).and_then(|body| str::from_utf8(body).ok())
            else {
                break;
            };
            lines.push(body.trim());
            above = line_start(text, comment.span.start);
        }
        if lines.is_empty() {
            return None;
        }
        lines.reverse();
        Some(lines.join("\n"))
    }

    /// Where its name is, in `encoding`'s positions.
    pub(crate) fn location(&self, encoding: PositionEncoding) -> Option<Location> {
        let uri = self.package.uri.clone()?;
        let before = &self.package.text()[..offset_to_index(self.name.end)];
        let range = LineIndex::new(before, encoding).range(before, self.name);
        Some(Location { uri, range })
    }
}

/// Where the line that holds `offset` starts.
fn line_start(text: &str, offset: u32) -> u32 {
    text[..offset_to_index(offset)].rfind('\n').map_or(0, |newline| index_to_offset(newline + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declaration_s_documentation_is_the_comments_directly_above_it() {
        let text = "\
-- Not this one.

--- Greets someone.
--[[ By name. ]]
--[==[ See t[1]. ]==]
declare function greet(name: string): string
declare app: { -- trailing
    -- Starts the app.
    start: () -> (),
    stop: () -> (),
}
";
        let packages = [Arc::new(Package::new("@app".to_owned(), None, Cow::Borrowed(text)))];
        let documentation = |symbol: &str| {
            let declaration =
                Declaration { documentation_symbol: Some(symbol.to_owned()), definition: None };
            Package::find(&packages, &declaration)?.documentation()
        };

        let greets = "\
Greets someone.
By name.
See t[1].";
        assert_eq!(documentation("@app/global/greet").as_deref(), Some(greets));
        assert_eq!(documentation("@app/global/app.start").as_deref(), Some("Starts the app."));
        assert_eq!(documentation("@app/global/app.stop"), None);
        assert_eq!(documentation("@app/global/app"), None);
        assert_eq!(
            documentation("@app/global/greet/overload/(string) -> string").as_deref(),
            Some(greets)
        );
    }

    #[test]
    fn every_checker_shares_one_index_of_roblox_s_api() {
        let declaration = Declaration {
            documentation_symbol: Some("@roblox/globaltype/Part.Anchored".to_owned()),
            definition: None,
        };
        let _ = Package::find(&[roblox_package(SecurityLevel::None)], &declaration);
        assert!(roblox_package(SecurityLevel::None).declarations.get().is_some());
    }
}
