//! Inlining a local read once: its value, a literal expression that can't
//! raise, moves to the read, which the printer parenthesizes as it needs.

use core::mem;

use luck_allocator::List;
use luck_ast::{
    Block, Expression, LocalAssignment, MAX_NESTING_DEPTH, Statement, Visit, VisitMut as _,
    expr_nesting_depth,
};
use luck_token::MAX_ACTIVE_LOCALS;

use super::Compressor;
use super::remove_dead_code::is_nil_initialized;
use super::simplify_parens::Place;
use crate::binding::{Bindings, NONE, Reference, has_fixed_binding, local_of};
use crate::expr::{MAX_ADDED_DEPTH, literal_kind, parenthesize};

/// An inlined local's initializer, waiting for the local's one read.
pub(super) struct Value<'ast> {
    expr: Expression<'ast>,
    /// The inlined locals it took in, its own included. Each is still in
    /// scope where its local is declared, so a chain past
    /// [`MAX_ACTIVE_LOCALS`] of them is longer than one function can hold,
    /// and it stops there.
    locals: usize,
}

/// Why a local read once keeps its value.
enum Refusal {
    /// More locals inlined into it than one function holds at once.
    Chain,
    /// It would nest past the parser's cap where it lands.
    Depth,
}

/// Why a local's value can't move to its one read, `read_depth` levels
/// deep, having taken in the values of `landed_locals` locals inlined into
/// it, if it can't.
fn refusal(value: &Expression<'_>, landed_locals: usize, read_depth: usize) -> Option<Refusal> {
    if landed_locals >= MAX_ACTIVE_LOCALS {
        return Some(Refusal::Chain);
    }
    let depth = expr_nesting_depth(value);
    // A Luau integer the value is lands in parentheses under `-`.
    let fits = depth <= MAX_ADDED_DEPTH
        && read_depth.saturating_add(1).saturating_add(depth) <= MAX_NESTING_DEPTH;
    (!fits).then_some(Refusal::Depth)
}

/// A binding's `held_value` once the walk refuses to move its value, which
/// the walk then leaves where it is.
const REFUSED: u32 = NONE - 1;

/// Removes the statement at `index`, keeping the others in order.
fn remove_stmt(stmts: &mut List<'_, Statement<'_>>, index: usize) {
    let kept = stmts.len().saturating_sub(1);
    for i in index..kept {
        stmts.swap(i, i.saturating_add(1));
    }
    stmts.truncate(kept);
}

/// Finds a read of one binding.
struct ReadFinder<'bindings> {
    bindings: &'bindings Bindings,
    id: usize,
    is_found: bool,
}

impl<'tree> Visit<'tree> for ReadFinder<'_> {
    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if self.is_found {
            return;
        }
        if let Expression::Name(name) = expr {
            self.is_found = local_of(name).is_some_and(|id| self.bindings.resolve(id) == self.id);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_stmt(&mut self, stmt: &'tree Statement<'tree>) {
        if !self.is_found {
            self.walk_stmt(stmt);
        }
    }
}

impl<'ast> Compressor<'_, 'ast> {
    /// Walks a `local` and drops each name read once whose value moves to
    /// the read, in a statement that gives each name its own value, and a
    /// `nil` initializer. Says whether anything of it stays.
    pub(super) fn visit_local(&mut self, stmt: &mut Statement<'ast>) -> bool {
        let Statement::LocalAssignment(local) = stmt else {
            return true;
        };
        let can_hold = !mem::take(&mut self.is_late_read);
        // An initializer can hold `local`s of its own, so the list is a
        // stack that every `local` leaves as it found it.
        let landed_start = self.landed_counts.len();
        for attributed in &mut local.names {
            self.visit_attributed_name(attributed);
        }
        for expr in &mut local.exprs {
            let outer_landed_locals = mem::replace(&mut self.landed_locals, 0);
            self.visit_expr(expr);
            let landed_locals = mem::replace(&mut self.landed_locals, outer_landed_locals);
            self.landed_counts.push(landed_locals);
        }
        let is_kept = self.inline_from(local, landed_start, can_hold);
        self.landed_counts.truncate(landed_start);
        is_kept
    }

    fn inline_from(
        &mut self,
        local: &mut LocalAssignment<'ast>,
        landed_start: usize,
        can_hold: bool,
    ) -> bool {
        if self.config.remove_dead_code && is_nil_initialized(local) {
            local.exprs = List::new();
            self.has_changed = true;
            return true;
        }
        // Dropping a name and its value keeps every other name's value only
        // when each name has its own.
        if !self.config.inline_locals
            || !can_hold
            || has_fixed_binding(local, self.version)
            || local.names.len() != local.exprs.len()
        {
            return true;
        }
        let mut kept = 0usize;
        for index in 0..local.names.len() {
            let (Some(attributed), Some(value), Some(&landed_locals)) = (
                local.names.get(index),
                local.exprs.get_mut(index),
                self.landed_counts.get(landed_start.saturating_add(index)),
            ) else {
                break;
            };
            if local_of(&attributed.name)
                .is_some_and(|id| self.hold_value(id, value, landed_locals))
            {
                self.retract_annotation(attributed);
                self.has_changed = true;
            } else {
                local.names.swap(kept, index);
                local.exprs.swap(kept, index);
                kept = kept.saturating_add(1);
            }
        }
        local.names.truncate(kept);
        local.exprs.truncate(kept);
        kept > 0
    }

    /// Inlines each local of `block` whose other reads went after the walk
    /// passed it, by walking the statement that holds its one read again
    /// with its value held, `outer_registers` being the registers around the
    /// block.
    pub(super) fn inline_late(&mut self, block: &mut Block<'ast>, outer_registers: usize) {
        while let Some((index, name_index, id)) = self.late_candidate(&block.stmts) {
            // A read in a `repeat`'s condition isn't the block's to walk.
            let Some(read_index) = self.find_read(block, index, id) else {
                self.needs_revisit = true;
                return;
            };
            let is_held = match block.stmts.get_mut(index) {
                Some(Statement::LocalAssignment(local)) => local
                    .exprs
                    .get_mut(name_index)
                    .is_some_and(|value| self.hold_value(id, value, 0)),
                _ => false,
            };
            if !is_held {
                continue;
            }
            self.live_registers =
                block.stmts.iter().take(read_index).fold(outer_registers, |registers, stmt| {
                    registers.saturating_add(super::merge_locals::declared_count(stmt))
                });
            let is_read_kept = block.stmts.get_mut(read_index).is_none_or(|stmt| {
                // The walk is past the reads of what the read's `local`
                // declares, so their values wait for a late inlining of
                // their own.
                self.is_late_read = matches!(stmt, Statement::LocalAssignment(_));
                let is_kept = self.visit_kept_stmt(stmt);
                self.is_late_read = false;
                is_kept
            });
            self.live_registers = outer_registers;
            if let Some(unlanded) = self.unland(id) {
                // Something kept the walk from the read, so the value goes
                // back for another walk.
                if let Some(Statement::LocalAssignment(declaration)) = block.stmts.get_mut(index)
                    && let Some(value) = declaration.exprs.get_mut(name_index)
                {
                    *value = unlanded;
                }
                self.needs_revisit = true;
                continue;
            }
            if !is_read_kept {
                remove_stmt(&mut block.stmts, read_index);
            }
            if let Some(Statement::LocalAssignment(declaration)) = block.stmts.get_mut(index) {
                if let Some(attributed) = declaration.names.get(name_index) {
                    self.retract_annotation(attributed);
                }
                let kept = declaration.names.len().saturating_sub(1);
                for i in name_index..kept {
                    declaration.names.swap(i, i.saturating_add(1));
                    declaration.exprs.swap(i, i.saturating_add(1));
                }
                declaration.names.truncate(kept);
                declaration.exprs.truncate(kept);
                if kept == 0 {
                    remove_stmt(&mut block.stmts, index);
                }
            }
            self.has_changed = true;
            if self.config.remove_dead_code {
                self.drop_unused(&mut block.stmts);
            }
        }
    }

    /// The index of the statement that holds the one read of the local
    /// `id`, declared by the statement at `index`.
    fn find_read(&self, block: &Block<'ast>, index: usize, id: usize) -> Option<usize> {
        let mut finder = ReadFinder { bindings: &*self.bindings, id, is_found: false };
        let later = block.stmts.iter().enumerate().skip(index.saturating_add(1));
        for (read_index, stmt) in later {
            finder.visit_stmt(stmt);
            if finder.is_found {
                return Some(read_index);
            }
        }
        None
    }

    /// The value of the local `id` if it's still held, which the walk then
    /// refuses to move.
    fn unland(&mut self, id: usize) -> Option<Expression<'ast>> {
        let binding = self.bindings.get_mut(id)?;
        let index = usize::try_from(binding.held_value).ok()?;
        let value = self.values.get_mut(index)?.take()?;
        binding.held_value = REFUSED;
        self.held_count = self.held_count.saturating_sub(1);
        Some(value.expr)
    }

    /// A local of `stmts` read once with no write, with a value that can
    /// move to the read, which the walk passed while it was read more: its
    /// statement's index, the name's index in it, and its binding.
    fn late_candidate(&self, stmts: &[Statement<'ast>]) -> Option<(usize, usize, usize)> {
        stmts.iter().enumerate().find_map(|(index, stmt)| {
            let Statement::LocalAssignment(local) = stmt else {
                return None;
            };
            if has_fixed_binding(local, self.version) || local.names.len() != local.exprs.len() {
                return None;
            }
            local.names.iter().zip(&local.exprs).enumerate().find_map(
                |(name_index, (attributed, value))| {
                    let id = local_of(&attributed.name)?;
                    let binding = self.bindings.get(id)?;
                    (binding.reads == 1
                        && binding.writes == 0
                        && binding.held_value != REFUSED
                        && literal_kind(value).is_some())
                    .then_some((index, name_index, id))
                },
            )
        })
    }

    /// Takes `value`, the initializer of the local `id`, to wait for the
    /// local's one read, if it has exactly one and no write, and the value
    /// can move there. Says whether it took it.
    pub(super) fn hold_value(
        &mut self,
        id: usize,
        value: &mut Expression<'ast>,
        landed_locals: usize,
    ) -> bool {
        let Some(facts) = self.bindings.get(id) else {
            return false;
        };
        if facts.reads != 1 || facts.writes != 0 || literal_kind(value).is_none() {
            return false;
        }
        let read_depth = usize::try_from(facts.read_depth).unwrap_or(usize::MAX);
        let held_value = match refusal(value, landed_locals, read_depth) {
            None => {
                let index = self.values.len();
                let locals = landed_locals.saturating_add(1);
                self.values.push(Some(Value { expr: value.take(), locals }));
                self.held_count = self.held_count.saturating_add(1);
                u32::try_from(index).unwrap_or(REFUSED)
            }
            Some(Refusal::Chain) => {
                self.needs_revisit = true;
                REFUSED
            }
            Some(Refusal::Depth) => {
                self.has_depth_refusal.set(true);
                REFUSED
            }
        };
        if let Some(binding) = self.bindings.get_mut(id) {
            binding.held_value = held_value;
        }
        held_value != REFUSED
    }

    /// An inlined local's value for its one read, which it reaches as the
    /// walk does, already rewritten.
    pub(super) fn take_value(&mut self, id: usize) -> Option<Expression<'ast>> {
        let binding = self.bindings.get_mut(id)?;
        let index = usize::try_from(binding.held_value).ok()?;
        let value = self.values.get_mut(index)?.take()?;
        binding.held_value = NONE;
        let read = Reference {
            is_write: false,
            function_depth: self.function_depth,
            type_floor: self.type_floor,
            depth: 0,
        };
        self.bindings.count(id, read, true);
        self.held_count = self.held_count.saturating_sub(1);
        self.landed_locals = self.landed_locals.saturating_add(value.locals);
        Some(value.expr)
    }

    /// Walks an operand at `place`, where a value that lands takes no
    /// parentheses but the ones that keep its meaning.
    pub(super) fn visit_operand(&mut self, operand: &mut Expression<'ast>, place: Place) {
        if let Expression::Name(name) = operand
            && self.held_count > 0
            && let Some(value) = local_of(name).and_then(|id| self.take_value(id))
        {
            self.has_landed_run |= matches!(value, Expression::BinaryOp(_));
            // Luau: `-1i` is a negative integer literal, while `-x` negates
            // the integer `x` holds, which raises.
            *operand = if place == Place::Negated && matches!(value, Expression::Integer(_)) {
                parenthesize(self.allocator, value)
            } else {
                value
            };
            return;
        }
        self.visit_expr_in(operand, place);
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_allocator::Allocator;
    use luck_ast::{Statement, expr_nesting_depth};
    use luck_token::LuaVersion;

    use crate::fixtures::{NO_PASSES, assert_outputs, compressed, minify_with, walk_once};
    use crate::transform_config::TransformConfig;

    const INLINING: TransformConfig = TransformConfig { inline_locals: true, ..NO_PASSES };

    fn inline(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &INLINING)
    }

    fn inline_luau(source: &str) -> String {
        minify_with(source, LuaVersion::Luau, &INLINING)
    }

    #[test]
    fn inlines_single_use_literals() {
        assert_outputs(
            inline,
            &[
                ("local x = 42 return x", "return 42"),
                ("local x = 42 local y = x + 1 return y", "return 42+1"),
                (
                    "local function foo() local x = 42 return x end return foo",
                    "local function foo()return 42 end return foo",
                ),
                ("if true then local x = 42 print(x) end", "if true then print(42)end"),
                ("local x = 1 print(x)", "print(1)"),
                ("local x = 1 local function f() return x end", "local function f()return 1 end"),
                ("do local x = 1 do print(x) end end", "do do print(1)end end"),
                ("repeat local x = 1 until x == y", "repeat until 1==y"),
                ("repeat local x = 1 repeat until x until z", "repeat repeat until 1 until z"),
            ],
        );
    }

    #[test]
    fn keeps_what_inlining_would_change() {
        for source in [
            // More than one read.
            "local x=42 print(x)return x",
            // A call, a read, a table, a closure, and `...` each carry an
            // effect or an identity.
            "local x=foo()return x",
            "local x=y return x",
            "local t={1,2,3}return t",
            "local f=function()return 1 end return f",
            "local function f(...)local x=...return x end",
            // A read outside the declaration's scope is a global's.
            "print(x)local x=1",
            "do local x=1 end print(x)",
            // A write would land on a global.
            "local x=1 x=2",
            "local x=\"s\"function x.f()end print(x)",
            // The error must still fire before the print.
            "local x=1+nil print(\"before\")return x",
        ] {
            assert_eq!(inline(source), source);
        }
        assert_eq!(inline_luau("local x=1 x+=2"), "local x=1 x+=2");
        assert_eq!(inline_luau("local x=foo::Bar return x"), "local x=foo::Bar return x");
    }

    #[test]
    fn keeps_a_name_another_binding_wears() {
        assert_eq!(
            inline("local self = 5 function M:get() return self end"),
            "local self=5 function M:get()return self end"
        );
        assert_eq!(
            minify_with(
                "local arg = 5 local function f(...) return arg end return f",
                LuaVersion::Lua51,
                &INLINING
            ),
            "local arg=5 local function f(...)return arg end return f"
        );
        for source in [
            "local args=1 local function f(...args)return args end",
            "local x=1 global x print(x)",
            "local x=1 global function g()local x=2 return x end return g",
        ] {
            let result = minify_with(source, LuaVersion::Lua55, &INLINING);
            assert!(!result.contains("return 1"), "{source}: {result}");
        }
    }

    #[test]
    fn inlines_each_of_two_locals_that_share_a_name() {
        assert_eq!(
            walk_once("local x = 1 print(x) do local x = 2 print(x) end", INLINING),
            "print(1)do print(2)end"
        );
    }

    #[test]
    fn inlines_a_local_whose_other_reads_the_walk_drops_after_passing_it() {
        let both = TransformConfig { remove_dead_code: true, fold_constants: true, ..INLINING };
        for source in [
            "local x = 1 print(x) if false then print(x) end",
            "local x = 1 print(x) print(false and x)",
            "local x = 1 print(x) local function f() return x end",
        ] {
            assert!(walk_once(source, both).starts_with("print(1)"), "{source}");
        }
    }

    #[test]
    fn inlines_a_local_whose_value_takes_a_late_inlined_one() {
        let both = TransformConfig { remove_dead_code: true, ..INLINING };
        assert_eq!(
            minify_with(
                "local a = 6 local b = a if false then print(a) end print(b)",
                LuaVersion::Lua54,
                &both
            ),
            "print(6)"
        );
    }

    #[test]
    fn uncounts_the_annotation_of_a_name_it_inlines() {
        let both = TransformConfig { remove_dead_code: true, ..INLINING };
        for (source, config) in [
            ("local a = 5 local b: typeof(a) = 1 print(a, b)", INLINING),
            ("local a = 5 local b: typeof(a) = 1 if false then print(b) end print(a, b)", both),
        ] {
            assert_eq!(minify_with(source, LuaVersion::Luau, &config), "print(5,1)", "{source}");
        }
    }

    #[test]
    fn parenthesizes_a_value_that_would_regroup_where_it_lands() {
        assert_outputs(
            inline,
            &[
                ("local s = 1 / 0 return 3 % s", "return 3%(1/0)"),
                ("local s = 1 + 2 return s * 2", "return(1+2)*2"),
                ("local s = 1 + 2 return 2 * s", "return 2*(1+2)"),
                ("local s = 1 - 2 return 5 - s", "return 5-(1-2)"),
                ("local s = 2 ^ 3 return s ^ 2", "return(2^3)^2"),
                ("local s = 1 .. 2 return s .. 3", "return(1 ..2)..3"),
                ("local s = 1 + 2 return 2 ^ s", "return 2^(1+2)"),
                ("local s = 1 or 2 return s and 3", "return(1 or 2)and 3"),
                ("local s = -2 return s ^ y", "return(-2)^y"),
                ("local s = not nil return s ^ y", "return(not nil)^y"),
                ("local s = 1 + 2 return -s", "return-(1+2)"),
                ("local s = 1 == 2 return not s", "return not(1==2)"),
                ("local s = \"a\" .. \"b\" return #s", "return#(\"a\"..\"b\")"),
                ("local s = 1 + 2 return -s ^ 2", "return-(1+2)^2"),
                ("local s = -1 return -s ^ 2", "return-(-1)^2"),
                ("local s = 1 + 2 return (s * 3)", "return(1+2)*3"),
                ("local s = 1 + 2 print(s * 3, 4)", "print((1+2)*3,4)"),
                ("local a = 1 + 2 local b = a * 3 return b ^ 2", "return((1+2)*3)^2"),
                ("local x = 37.5 return x[\"k\"]", "return(37.5)[\"k\"]"),
                ("local x = \"s\" return x.len", "return(\"s\").len"),
                ("local x = false x:resize(9)", "(false):resize(9)"),
            ],
        );
    }

    #[test]
    fn leaves_a_value_bare_where_its_grouping_holds() {
        assert_outputs(
            inline,
            &[
                ("local s = 1 + 2 return s", "return 1+2"),
                ("local s = 1 + 2 print(s)", "print(1+2)"),
                ("local s = 1 + 2 return {s}", "return{1+2}"),
                ("local s = 1 + 2 return t[s]", "return t[1+2]"),
                ("local s = 1 * 2 return s + 3", "return 1*2+3"),
                ("local s = 1 * 2 return 3 + s", "return 3+1*2"),
                ("local s = 1 - 2 return s - 5", "return 1-2-5"),
                ("local s = 2 ^ 3 return 2 ^ s", "return 2^2^3"),
                ("local s = 1 .. 2 return 3 .. s", "return 3 ..1 ..2"),
                ("local s = 1 + 2 return s < 4", "return 1+2<4"),
                ("local s = 1 < 2 return s == true", "return 1<2==true"),
                ("local s = 2 ^ 3 return -s", "return-2^3"),
                ("local s = -2 return 3 ^ s", "return 3^-2"),
                ("local s = -2 return s * 3", "return-2*3"),
                ("local s = -1 return -s", "return- -1"),
                ("local s = (1 + 2) return s * 2", "return(1+2)*2"),
            ],
        );
    }

    #[test]
    fn inlines_into_luau_casts_and_instantiations() {
        assert_outputs(
            inline_luau,
            &[
                ("local s = 1 + 2 return s :: any", "return(1+2)::any"),
                ("local s = -1 return s :: any", "return(-1)::any"),
                ("local s = 1 :: number return s :: any", "return(1::number)::any"),
                ("local s = 1 return s :: any", "return 1::any"),
                ("local s = 1 :: number return s < 2", "return(1::number)<2"),
                ("local s = 1 :: number return 2 + s < 3", "return 2+(1::number)<3"),
                ("local s = 1 :: number return -s < 2", "return-(1::number)<2"),
                ("local s = 1 :: number return 1 < s < 3", "return 1<(1::number)<3"),
                ("local s = 2 + 1 :: number return s < 3", "return 2+(1::number)<3"),
                ("local s = -1 :: number return s < 3", "return-(1::number)<3"),
                // The cast ends inside the sum's parentheses, before `)`.
                ("local s = 2 + 1 :: number return 2 * s < 3", "return 2*(2+1::number)<3"),
                ("local s = 1 :: number return s", "return 1::number"),
                ("local s = 1 :: number return s + 1", "return 1::number+1"),
                ("local s = 1 :: number return 1 + s", "return 1+1::number"),
                ("local s = 1 :: number return s > 2", "return 1::number>2"),
                ("local s = 1 return x :: Foo<T> < s", "return x::Foo<T><1"),
                ("local s = \"x\" return s<<T>>", "return(\"x\")<<T>>"),
                ("local s = -1 return s<<T>>", "return(-1)<<T>>"),
                ("local s = \"x\" :: T return s<<U>>", "return(\"x\"::T)<<U>>"),
                ("local s = \"x\" return s<<T>><<U>>", "return(\"x\")<<T>><<U>>"),
                // `-1i` is a literal, while negating an integer raises.
                ("local s = 1i return -s", "return-(1i)"),
                ("local s = 1i return not s", "return not 1i"),
                ("local s = (\"x\")<<T>> return s .. \"y\"", "local s=(\"x\")<<T>>return s..\"y\""),
            ],
        );
    }

    /// `local x0 = 1`, then `local xi = x(i-1) + 1` up to `x(links)`, returned.
    fn sum_chain(links: usize) -> String {
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..=links).zip(0usize..) {
            writeln!(source, "local x{i} = x{previous} + 1").unwrap();
        }
        writeln!(source, "return x{links}").unwrap();
        source
    }

    #[test]
    fn inlines_a_chain_of_as_many_locals_as_one_function_holds() {
        let links = luck_token::MAX_ACTIVE_LOCALS - 1;
        let allocator = Allocator::new();
        let source = sum_chain(links);
        let chunk = compressed(&allocator, &source, INLINING);
        let [Statement::Return(return_stmt)] = &*chunk.block.stmts else {
            panic!("expected a return");
        };
        let run = return_stmt.exprs.first().expect("the return has its value");
        assert_eq!(expr_nesting_depth(run), 1, "the chain splices into one run");
        assert_eq!(
            luck_codegen::compact(&chunk),
            format!("return 1{}", "+1".repeat(links)).into_bytes()
        );

        let mut concat = String::from("local x0 = \"z\"\n");
        for (i, previous) in (1..=links).zip(0usize..) {
            writeln!(concat, "local x{i} = \"a\" .. x{previous}").unwrap();
        }
        writeln!(concat, "return x{links}").unwrap();
        assert_eq!(inline(&concat), format!("return{}\"z\"", "\"a\"..".repeat(links)));
    }

    #[test]
    fn stops_a_chain_past_the_locals_one_function_holds() {
        let links = luck_token::MAX_ACTIVE_LOCALS;
        let expected = format!("local x{links}=1{} return x{links}", "+1".repeat(links));
        assert_eq!(walk_once(&sum_chain(links), INLINING), expected);
    }

    #[test]
    fn stops_inlining_a_run_of_locals_before_it_nests_past_the_depth_cap() {
        // Each value needs parentheses where the next one uses it.
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..luck_token::MAX_ACTIVE_LOCALS).zip(0usize..) {
            writeln!(source, "local x{i} = x{previous} * 2 + 1").unwrap();
        }
        writeln!(source, "return x{}", luck_token::MAX_ACTIVE_LOCALS - 1).unwrap();
        assert!(inline(&source).contains("local"), "the run must stop inlining partway");
    }
}
