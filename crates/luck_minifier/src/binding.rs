//! Binding identity, which the bind walk writes into the tree, and what the
//! passes know of each binding.
//!
//! Every name that declares or refers to a local holds the local's id in its
//! index, and every name that refers to a global holds the global's name id,
//! so a pass reads identity from the name itself however the tree has been
//! edited, moved, or built, and two locals that share a name never meet.

use core::mem;

use luck_ast::{
    Expression, FunctionBody, LocalAssignment, LocalAssignmentKind, LocalFunction, Name, NameIndex,
    Statement, Type, Var, Visitor,
};
use luck_token::{LuaVersion, index_to_offset, offset_to_index};

pub(crate) const METHOD_RECEIVER_NAME: &str = "self";

pub(crate) const IMPLICIT_VARARG_TABLE_NAME: &str = "arg";

/// The name ids the bind walk gives [`METHOD_RECEIVER_NAME`] and
/// [`IMPLICIT_VARARG_TABLE_NAME`] before any other.
pub(crate) const RECEIVER_NAME_ID: usize = 0;
pub(crate) const VARARG_TABLE_NAME_ID: usize = 1;

/// Where the indexes of locals and of globals start. The parser's are below
/// both, so a name the bind walk never reached holds neither.
const LOCAL_INDEX: u32 = 1 << 31;
const GLOBAL_INDEX: u32 = 3 << 30;

/// The local `name` declares or refers to, before any merge.
pub(crate) fn local_of(name: &Name<'_>) -> Option<usize> {
    let index = name.index?.get();
    (LOCAL_INDEX..GLOBAL_INDEX)
        .contains(&index)
        .then(|| offset_to_index(index.wrapping_sub(LOCAL_INDEX)))
}

/// The name id of the global `name` refers to.
pub(crate) fn global_of(name: &Name<'_>) -> Option<usize> {
    name.index?.get().checked_sub(GLOBAL_INDEX).map(offset_to_index)
}

/// The index of a name that declares or refers to the local `id`.
pub(crate) fn local_index(id: usize) -> NameIndex {
    let index = LOCAL_INDEX.saturating_add(index_to_offset(id));
    NameIndex::new(index.min(GLOBAL_INDEX.wrapping_sub(1)))
}

/// The index of a name that refers to the global with name id `name_id`.
pub(crate) fn global_index(name_id: usize) -> NameIndex {
    NameIndex::new(GLOBAL_INDEX.saturating_add(index_to_offset(name_id)))
}

pub(crate) const fn has_implicit_arg(body: &FunctionBody<'_>, version: LuaVersion) -> bool {
    body.vararg.is_some() && version.has_implicit_vararg_table()
}

/// Whether this name is the `_ENV` upvalue every global read and write
/// goes through. Outside 5.2+ the name binds an ordinary local, so
/// pinning it would cost bytes for nothing.
pub(crate) fn is_env_binding(name: &str, version: LuaVersion) -> bool {
    version.has_env_upvalue() && name == "_ENV"
}

/// Whether a `local` declaration binds a name with an attribute (`<close>`
/// runs `__close` at scope exit and `<const>` affects validity) or a 5.2+
/// `_ENV`, which redirects global lookup for everything after it.
pub(crate) fn has_attributed_or_env_name(local: &LocalAssignment<'_>, version: LuaVersion) -> bool {
    local.names.iter().any(|attributed| {
        attributed.attribute.is_some() || is_env_binding(attributed.name.text.as_str(), version)
    })
}

/// Whether no transform may reshape, merge away, or move a `local`
/// declaration: an export, a `const`, or one that
/// [`has_attributed_or_env_name`].
pub(crate) fn has_fixed_binding(local: &LocalAssignment<'_>, version: LuaVersion) -> bool {
    local.kind != LocalAssignmentKind::Local || has_attributed_or_env_name(local, version)
}

pub(crate) fn is_droppable_function(
    local_function: &LocalFunction<'_>,
    version: LuaVersion,
) -> bool {
    !local_function.kind.is_exported()
        && !is_env_binding(local_function.name.text.as_str(), version)
}

/// What every pass knows of one local.
#[derive(Clone, Copy)]
pub(crate) struct Binding {
    pub name: u32,
    /// The binding a lift merged this one into, or its own id.
    pub merged_into: u32,
    /// Keeps its name: an export, a 5.2+ `_ENV`, or an implicit `self` or
    /// 5.1 `arg`.
    pub is_fixed: bool,
    /// A `local function` whose body a walk is in, where a reference to it
    /// is its own and keeps nothing alive.
    pub is_open: bool,
    /// How many functions deep it's declared.
    pub function_depth: u32,
    /// Its reads and writes, and its references from functions nested in its
    /// own, in the tree as it stands.
    pub reads: u32,
    pub writes: u32,
    pub captures: u32,
    /// How deep its latest read sits, which for a binding read once is
    /// where an inlined value lands.
    pub read_depth: u32,
    /// The compressor's held value for it, an index into its values.
    pub held_value: u32,
}

pub(crate) const NONE: u32 = u32::MAX;

/// Every binding of a tree by id, and what's known of each global name.
#[derive(Default)]
pub(crate) struct Bindings {
    pub entries: Vec<Binding>,
    /// References of each global name outside types, and captures of the
    /// bindings of each name, which a lift that keeps names reads.
    pub global_uses: Vec<u32>,
    pub name_captures: Vec<u32>,
    /// Lua 5.5: `global` declarations of each name, which keep it from
    /// `rename_globals`.
    pub global_declarations: Vec<u32>,
}

impl Bindings {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.global_uses.clear();
        self.name_captures.clear();
        self.global_declarations.clear();
    }

    pub(crate) fn push(&mut self, name: usize, is_fixed: bool, function_depth: u32) -> usize {
        let id = self.entries.len();
        self.entries.push(Binding {
            name: u32::try_from(name).unwrap_or(NONE),
            merged_into: u32::try_from(id).unwrap_or(NONE),
            is_fixed,
            is_open: false,
            function_depth,
            reads: 0,
            writes: 0,
            captures: 0,
            read_depth: 0,
            held_value: NONE,
        });
        id
    }

    /// The binding `id` stands for once merges are followed.
    pub(crate) fn resolve(&self, id: usize) -> usize {
        self.entries
            .get(id)
            .map_or(id, |binding| usize::try_from(binding.merged_into).unwrap_or(id))
    }

    pub(crate) fn get(&self, id: usize) -> Option<&Binding> {
        self.entries.get(self.resolve(id))
    }

    pub(crate) fn get_mut(&mut self, id: usize) -> Option<&mut Binding> {
        let resolved = self.resolve(id);
        self.entries.get_mut(resolved)
    }

    /// Whether code the tree keeps reads or writes `id`, a reference of a
    /// `local function` to itself aside.
    pub(crate) fn is_used(&self, id: usize) -> bool {
        self.get(id).is_some_and(|binding| binding.reads > 0 || binding.writes > 0)
    }

    pub(crate) fn resize_names(&mut self, name_count: usize) {
        if self.global_uses.len() < name_count {
            self.global_uses.resize(name_count, 0);
            self.name_captures.resize(name_count, 0);
            self.global_declarations.resize(name_count, 0);
        }
    }

    /// Counts, or with `is_retracted` uncounts, a reference to `id` from
    /// `function_depth` functions deep, where a type that holds it puts a
    /// floor under the bindings it can capture.
    pub(crate) fn count(&mut self, id: usize, reference: Reference, is_retracted: bool) {
        let Some(binding) = self.get_mut(id) else {
            return;
        };
        // A reference in a type captures nothing declared outside it, since
        // types never run.
        let floor = reference.type_floor;
        let is_capture = binding.is_open
            || (reference.function_depth > binding.function_depth
                && binding.function_depth >= floor);
        if !binding.is_open {
            if reference.is_write {
                step(&mut binding.writes, is_retracted);
            } else {
                step(&mut binding.reads, is_retracted);
                if !is_retracted {
                    binding.read_depth = reference.depth;
                }
            }
        }
        if is_capture {
            step(&mut binding.captures, is_retracted);
            let name = usize::try_from(binding.name).unwrap_or(usize::MAX);
            if let Some(count) = self.name_captures.get_mut(name) {
                step(count, is_retracted);
            }
        }
    }

    pub(crate) fn count_global(&mut self, name: usize, reference: Reference, is_retracted: bool) {
        if reference.type_floor == 0
            && let Some(count) = self.global_uses.get_mut(name)
        {
            step(count, is_retracted);
        }
    }

    /// Counts, or uncounts, a reference by `name`, whatever it refers to.
    pub(crate) fn count_name(&mut self, name: &Name<'_>, reference: Reference, is_retracted: bool) {
        match (local_of(name), global_of(name)) {
            (Some(id), _) => self.count(id, reference, is_retracted),
            (None, Some(global)) => self.count_global(global, reference, is_retracted),
            (None, None) => {}
        }
    }

    pub(crate) fn count_global_declaration(&mut self, name: &Name<'_>, is_retracted: bool) {
        if let Some(count) = global_of(name).and_then(|id| self.global_declarations.get_mut(id)) {
            step(count, is_retracted);
        }
    }
}

const fn step(count: &mut u32, is_retracted: bool) {
    *count = if is_retracted { count.saturating_sub(1) } else { count.saturating_add(1) };
}

/// Where a reference sits.
#[derive(Clone, Copy)]
pub(crate) struct Reference {
    pub is_write: bool,
    pub function_depth: u32,
    /// One past the function depth of the innermost type around it, or 0.
    pub type_floor: u32,
    pub depth: u32,
}

/// Counts every reference in a tree that already holds its identities, or
/// uncounts those in a part the compressor drops.
pub(crate) struct Counter<'bindings> {
    pub bindings: &'bindings mut Bindings,
    pub is_retracting: bool,
    pub function_depth: u32,
    pub type_floor: u32,
    pub depth: u32,
}

impl Counter<'_> {
    fn reference(&mut self, name: &Name<'_>, is_write: bool) {
        let reference = Reference {
            is_write,
            function_depth: self.function_depth,
            type_floor: self.type_floor,
            depth: self.depth,
        };
        self.bindings.count_name(name, reference, self.is_retracting);
    }

    fn set_open(&mut self, name: &Name<'_>, is_open: bool) {
        if let Some(binding) = local_of(name).and_then(|id| self.bindings.get_mut(id)) {
            binding.is_open = is_open;
        }
    }
}

impl<'tree> Visitor<'tree> for Counter<'_> {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        true
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_stmt(&mut self, stmt: &'tree Statement<'tree>) {
        match stmt {
            Statement::FunctionDecl(function_decl) => {
                if let Some(root) = function_decl.name.names.first() {
                    self.reference(root, true);
                }
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global) => {
                for attributed in &global.names {
                    self.bindings.count_global_declaration(&attributed.name, self.is_retracting);
                }
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.bindings.count_global_declaration(&global_function.name, self.is_retracting);
            }
            Statement::LocalFunction(local_function) => {
                self.set_open(&local_function.name, true);
                self.walk_stmt(stmt);
                self.set_open(&local_function.name, false);
                return;
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if let Expression::Name(name) = expr {
            self.reference(name, false);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &'tree Var<'tree>) {
        if let Var::Name(name) = target {
            self.reference(name, true);
            return;
        }
        self.walk_assignment_target(target);
    }

    /// A signature belongs to the scope around the function, as the bind
    /// walk counts it.
    fn visit_function_body(&mut self, body: &'tree FunctionBody<'tree>) {
        if self.descend() {
            if let Some(generics) = &body.generics {
                self.visit_generic_type_list(generics);
            }
            for param in &body.params {
                if let Some(ty) = &param.type_annotation {
                    self.visit_type(ty);
                }
            }
            if let Some(ty) =
                body.vararg.as_ref().and_then(|vararg| vararg.type_annotation.as_ref())
            {
                self.visit_type(ty);
            }
            if let Some(ty) = &body.return_type {
                self.visit_type(ty);
            }
            self.function_depth = self.function_depth.saturating_add(1);
            self.visit_block(&body.block);
            self.function_depth = self.function_depth.saturating_sub(1);
        }
        self.ascend();
    }

    fn visit_type(&mut self, ty: &'tree Type<'tree>) {
        let outer_floor = mem::replace(&mut self.type_floor, self.function_depth.saturating_add(1));
        self.walk_type(ty);
        self.type_floor = outer_floor;
    }
}
