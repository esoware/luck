//! What a file's requires load, and what its own table holds.
//!
//! A module's table is what its chunk ends by returning: a local nothing
//! reassigns, as in `return M`, a table constructor, or in Luau, the table of
//! the locals it exports. A member of it is a field the module reads or
//! writes on that local, a key of the constructor it returns or initializes
//! the local with, or an exported local. A member of a required module's
//! table is a field read or written on the call itself, as in
//! `require("m").field`, or on a local the call initializes and nothing
//! reassigns.
//!
//! A field reached through anything else, such as `self` or a value a call
//! returned, is no member, since only running the program says what table
//! holds it.

use ls_types::{Position, Range};
use luck_ast::{
    Assignment, Chunk, Expression, Field, FunctionDecl, FunctionName, LocalAssignment, Name,
    Statement, Suffix, SuffixedExpression, TableConstructor, Var, Visit,
};
use luck_semantic::{Scoping, Semantic, SymbolId};
use luck_token::Span;
use rustc_hash::FxHashMap;

use crate::providers::cursor::identifier;

/// What one file shows the others: the modules it requires by path, and
/// each place it names a member of a module's table.
#[derive(Debug)]
pub(crate) struct ModuleRecord {
    /// In source order.
    requires: Vec<Require>,
    /// In source order.
    members: Vec<Member>,
}

/// A call of the host's `require` with a string literal, which no standard
/// library answers.
#[derive(Debug)]
pub(crate) struct Require {
    /// As `require` receives it, escapes decoded.
    pub module: Box<str>,
    /// The string literal.
    pub span: Span,
    pub range: Range,
}

/// A place a file names a member of a module's table.
#[derive(Debug)]
pub(crate) struct Member {
    pub table: Table,
    pub name: Box<str>,
    pub range: Range,
    /// Whether it gives the member of the file's own table its value: a key
    /// of a constructor, a `function M.name` declaration, a plain assignment
    /// to `M.name`, or an exported local's declaration.
    pub is_definition: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Table {
    /// The table the file returns.
    Returned,
    /// Luau: the table of the locals the file exports, where this one is a
    /// local.
    Exported,
    /// The table the require at this index loads.
    Required(usize),
}

impl ModuleRecord {
    /// The record of `chunk`, whose `semantic` holds its `require` sites,
    /// with `range` placing each span.
    pub(crate) fn new(
        chunk: &Chunk<'_>,
        semantic: &Semantic<'_>,
        mut range: impl FnMut(Span) -> Range,
    ) -> Self {
        let scoping = semantic.scoping();
        let mut requires = Vec::new();
        let mut sites = FxHashMap::default();
        for site in semantic.requires() {
            let Some(path) = site.path else {
                continue;
            };
            let Some(module) = path
                .as_str()
                .filter(|module| !luck_stdlib::is_preloaded_module(module, chunk.version))
            else {
                continue;
            };
            sites.insert(site.call.callee.start, (requires.len(), site.call.callee.suffixes.len()));
            requires.push(Require {
                module: module.into(),
                span: path.span,
                range: range(path.span),
            });
        }
        let returned = chunk.block.stmts.last().and_then(|last| {
            let Statement::Return(return_stmt) = last else {
                return None;
            };
            let [returned] = &*return_stmt.exprs else {
                return None;
            };
            Some(returned)
        });
        let mut collector = Collector {
            scoping,
            sites,
            returned: returned.and_then(|expr| returned_local(scoping, expr)),
            bindings: FxHashMap::default(),
            found: Vec::new(),
        };
        if let Some(Expression::TableConstructor(table)) = returned {
            collector.keys(table);
        }
        collector.visit_chunk(chunk);
        for symbol in scoping.symbols().iter().filter(|symbol| symbol.is_exported) {
            let name = scoping.name(symbol.name);
            collector.found.push(Found {
                table: Table::Exported,
                name,
                span: symbol.definition_span,
                is_definition: true,
            });
            collector.found.extend(scoping.resolved_references(symbol.id).iter().map(|&id| {
                Found {
                    table: Table::Exported,
                    name,
                    span: scoping.reference(id).span,
                    is_definition: false,
                }
            }));
        }
        let mut found = collector.found;
        found.sort_by_key(|member| member.span.start);
        let members = found
            .into_iter()
            .map(|member| Member {
                table: member.table,
                name: member.name.into(),
                range: range(member.span),
                is_definition: member.is_definition,
            })
            .collect();
        Self { requires, members }
    }

    pub(crate) fn requires(&self) -> &[Require] {
        &self.requires
    }

    pub(crate) fn members(&self) -> &[Member] {
        &self.members
    }

    /// The member named at `position`, either end included.
    pub(crate) fn member_at(&self, position: Position) -> Option<&Member> {
        let after = self.members.partition_point(|member| member.range.start <= position);
        let member = self.members.get(after.checked_sub(1)?)?;
        (position <= member.range.end).then_some(member)
    }
}

/// The local `expr`, what the chunk returns, names, when nothing reassigns
/// it.
fn returned_local(scoping: &Scoping, expr: &Expression<'_>) -> Option<SymbolId> {
    let Expression::Name(name) = expr else {
        return None;
    };
    let symbol = scoping.reference(scoping.reference_of(name)?).resolved?;
    (!scoping.symbol(symbol).is_modified).then_some(symbol)
}

/// The name a member suffix, `.name` or `:name(...)`, reads.
fn member_name<'ast>(suffix: &'ast Suffix<'ast>) -> Option<&'ast Name<'ast>> {
    let name = match suffix {
        Suffix::Field(name) => name,
        Suffix::MethodCall(call) => &call.name,
        Suffix::Index { .. } | Suffix::Call(_) | Suffix::TypeInstantiation { .. } => return None,
    };
    identifier(name).map(|_| name)
}

struct Found<'ast> {
    table: Table,
    name: &'ast str,
    span: Span,
    is_definition: bool,
}

struct Collector<'ast> {
    scoping: &'ast Scoping,
    /// Each require in the record, by where its chain starts, with its
    /// index and how many suffixes come before its call.
    sites: FxHashMap<u32, (usize, usize)>,
    /// The local the chunk returns.
    returned: Option<SymbolId>,
    /// Each local a require's call initializes, with the require's index.
    bindings: FxHashMap<SymbolId, usize>,
    found: Vec<Found<'ast>>,
}

impl<'ast> Collector<'ast> {
    /// The table whose member a field of `name` is.
    fn table_of(&self, name: &Name<'_>) -> Option<Table> {
        let symbol = self.scoping.reference(self.scoping.reference_of(name)?).resolved?;
        if Some(symbol) == self.returned {
            return Some(Table::Returned);
        }
        self.bindings.get(&symbol).map(|&require| Table::Required(require))
    }

    /// A definition counts only in the file's own table.
    fn push(&mut self, table: Table, name: &'ast Name<'ast>, is_definition: bool) {
        self.found.push(Found {
            table,
            name: name.text.as_str(),
            span: name.span,
            is_definition: is_definition && table == Table::Returned,
        });
    }

    /// The member `chain` names, which `is_target` says it assigns.
    fn chain(&mut self, chain: &'ast SuffixedExpression<'ast>, is_target: bool) {
        let named = if let Expression::Name(prefix) = &chain.prefix
            && let Some(table) = self.table_of(prefix)
        {
            Some((table, 0))
        } else {
            self.sites.get(&chain.span.start).map(|&(require, before_call)| {
                (Table::Required(require), before_call.saturating_add(1))
            })
        };
        let Some((table, at)) = named else {
            return;
        };
        if let Some(name) = chain.suffixes.get(at).and_then(member_name) {
            self.push(table, name, is_target && at.saturating_add(1) == chain.suffixes.len());
        }
    }

    /// `function M.name()` and `function M:name()` define a member, and
    /// `function M.name.other()` reads one.
    fn function_name(&mut self, function_name: &'ast FunctionName<'ast>) {
        let Some(table) = function_name.names.first().and_then(|root| self.table_of(root)) else {
            return;
        };
        let (name, is_definition) = match (function_name.names.get(1), &function_name.method) {
            (Some(name), method) => (name, function_name.names.len() == 2 && method.is_none()),
            (None, Some(method)) => (method, true),
            (None, None) => return,
        };
        if identifier(name).is_some() {
            self.push(table, name, is_definition);
        }
    }

    /// The named keys of `table`, the file's own table.
    fn keys(&mut self, table: &'ast TableConstructor<'ast>) {
        for field in &table.fields {
            if let Field::Named { name, .. } = field
                && identifier(name).is_some()
            {
                self.push(Table::Returned, name, true);
            }
        }
    }

    /// The locals `local_assignment` binds to a require's table, and the
    /// keys it gives the returned local.
    fn bind(&mut self, local_assignment: &'ast LocalAssignment<'ast>) {
        for (attributed, expr) in local_assignment.names.iter().zip(&local_assignment.exprs) {
            let Some(symbol) = self.scoping.symbol_of(&attributed.name) else {
                continue;
            };
            if self.scoping.symbol(symbol).is_modified {
                continue;
            }
            if let Expression::Suffixed(chain) = expr
                && let Some(&(require, before_call)) = self.sites.get(&chain.span.start)
                && before_call.saturating_add(1) == chain.suffixes.len()
            {
                self.bindings.insert(symbol, require);
            }
            if let Expression::TableConstructor(table) = expr
                && Some(symbol) == self.returned
            {
                self.keys(table);
            }
        }
    }
}

impl<'ast> Visit<'ast> for Collector<'ast> {
    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        self.chain(suffixed_expr, false);
        self.walk_suffixed_expr(suffixed_expr);
    }

    fn visit_assignment(&mut self, assignment: &'ast Assignment<'ast>) {
        for target in &assignment.targets {
            if let Var::Suffixed(chain) = target {
                self.chain(chain, true);
                self.walk_suffixed_expr(chain);
            }
        }
        for value in &assignment.values {
            self.visit_expr(value);
        }
    }

    fn visit_function_decl(&mut self, function_decl: &'ast FunctionDecl<'ast>) {
        self.function_name(&function_decl.name);
        self.walk_function_decl(function_decl);
    }

    fn visit_local_assignment(&mut self, local_assignment: &'ast LocalAssignment<'ast>) {
        self.walk_local_assignment(local_assignment);
        self.bind(local_assignment);
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_semantic::SemanticBuilder;
    use luck_token::LuaVersion;

    use super::*;

    /// Each member of `source`'s record as its table, name, and whether it's
    /// a definition, and each require's module.
    fn record_of(source: &str, version: LuaVersion) -> (Vec<(Table, &str, bool)>, Vec<String>) {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes(), version);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let chunk = allocator.alloc(parsed.chunk);
        let semantic = SemanticBuilder::new().with_requires(true).build(chunk).semantic;
        let record = ModuleRecord::new(chunk, &semantic, |span| Range {
            start: Position::new(0, span.start),
            end: Position::new(0, span.end),
        });
        let members = record
            .members()
            .iter()
            .map(|member| {
                let start = usize::try_from(member.range.start.character).expect("a small offset");
                let end = start.saturating_add(member.name.len());
                let name = source.get(start..end).expect("in the source");
                assert_eq!(name, &*member.name);
                (member.table, name, member.is_definition)
            })
            .collect();
        let modules = record.requires().iter().map(|require| require.module.to_string()).collect();
        (members, modules)
    }

    #[test]
    fn a_returned_local_s_fields_and_keys_are_its_members() {
        let (members, _) = record_of(
            "\
local M = { version = 1 }
function M.new() return setmetatable({}, M) end
function M:describe() return M.version end
M.count = 0
M.inner.deep = 1
local function bump() M.count = M.count + 1 end
return M",
            LuaVersion::Lua54,
        );
        assert_eq!(
            members,
            [
                (Table::Returned, "version", true),
                (Table::Returned, "new", true),
                (Table::Returned, "describe", true),
                (Table::Returned, "version", false),
                (Table::Returned, "count", true),
                (Table::Returned, "inner", false),
                (Table::Returned, "count", true),
                (Table::Returned, "count", false),
            ]
        );
    }

    #[test]
    fn a_returned_constructor_s_keys_are_the_members() {
        let (members, _) =
            record_of("return { first = 1, [\"second\"] = 2, 3 }", LuaVersion::Lua54);
        assert_eq!(members, [(Table::Returned, "first", true)]);
    }

    #[test]
    fn a_reassigned_or_unreturned_local_has_no_members() {
        for source in [
            "\
local M = {}
M.a = 1
M = {}
return M",
            "\
local M = {}
M.a = 1
return M, 2",
            "\
local M = {}
M.a = 1",
        ] {
            assert_eq!(record_of(source, LuaVersion::Lua54).0, [], "{source}");
        }
    }

    #[test]
    fn a_field_of_a_require_or_its_binding_is_a_member_of_its_table() {
        let (members, modules) = record_of(
            "\
local util = require(\"util\")
local other = require(\"other\")
local string = require(\"string\")
util.run(util.config.level, other:make())
require(\"direct\").go()
_G.require(\"global\").field = 1
util.patched = true
other = nil
other.gone()
string.format()",
            LuaVersion::Lua54,
        );
        assert_eq!(modules, ["util", "other", "direct", "global"]);
        assert_eq!(
            members,
            [
                (Table::Required(0), "run", false),
                (Table::Required(0), "config", false),
                (Table::Required(2), "go", false),
                (Table::Required(3), "field", false),
                (Table::Required(0), "patched", false),
            ],
            "`other` is reassigned, and `string` is the standard library"
        );
    }

    #[test]
    fn a_luau_export_and_its_reads_are_members_of_the_export_table() {
        let (members, _) = record_of(
            "\
export local version = 1
export function describe() return version end",
            LuaVersion::Luau,
        );
        assert_eq!(
            members,
            [
                (Table::Exported, "version", true),
                (Table::Exported, "describe", true),
                (Table::Exported, "version", false),
            ]
        );
    }

    #[test]
    fn member_at_finds_the_name_under_either_end() {
        let allocator = Allocator::new();
        let source = "local m = require(\"m\") m.run()";
        let parsed = luck_parser::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        let chunk = allocator.alloc(parsed.chunk);
        let semantic = SemanticBuilder::new().with_requires(true).build(chunk).semantic;
        let record = ModuleRecord::new(chunk, &semantic, |span| Range {
            start: Position::new(0, span.start),
            end: Position::new(0, span.end),
        });
        let name_at = |character| {
            record.member_at(Position::new(0, character)).map(|member| member.name.to_string())
        };
        assert_eq!(name_at(25).as_deref(), Some("run"));
        assert_eq!(name_at(28).as_deref(), Some("run"));
        assert_eq!(name_at(23), None);
        assert_eq!(name_at(29), None);
    }
}
