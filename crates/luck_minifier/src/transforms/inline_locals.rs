use core::{iter, mem};

use luck_ast::{
    BinaryOp, Block, Expression, FunctionBody, LocalAssignment, MAX_NESTING_DEPTH, Statement,
    Suffix, SuffixedExpression, Transform, Var, Visitor, expr_nesting_depth, is_prefix_expr,
    type_nesting_depth,
};
use luck_token::{Associativity, BinOp, CompactString, LuaVersion, OperandSide};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::binding::{
    IMPLICIT_VARARG_TABLE_NAME, METHOD_RECEIVER_NAME, has_fixed_binding, has_implicit_arg,
    identifier_name,
};
use crate::expr::{
    LiteralKind, MAX_ADDED_DEPTH, binary_literal_kind, binds_looser_than_unary, inner_operand_side,
    is_nonzero_number_literal, literal_kind, parenthesize, unary_literal_kind,
};

/// This pass works on names, not bindings, so every candidate must be
/// position-insensitive:
/// - the name must not be bound anywhere else in the file (by another local,
///   a parameter, a loop variable, a `local function`, a 5.5 `global`, or a
///   function's implicit `self` or 5.1 `arg`), or a use of that binding would
///   get the candidate's value;
/// - every reference must be a read in the declaration's scope, since a read
///   before or past it is a global's and a write would become one;
/// - the value must be one no error or identity depends on: an operator over
///   literals may raise (`1 + nil` keeps its error before later statements),
///   a table or closure moved into a loop mints a fresh object per iteration,
///   and `...` means something else across a function boundary.
///
/// One scan finds the names that qualify, and one walk decides each where
/// it's declared, with the values declared before it already in place, so a
/// chain of locals inlines whole. A value moves to its one read, and nothing
/// walks it again.
pub(crate) fn inline(block: Block, version: LuaVersion) -> Block {
    let candidates = find_inline_candidates(&block, version);
    if candidates.is_empty() {
        return block;
    }
    let mut inliner = Inliner {
        candidates,
        values: FxHashMap::default(),
        is_in_value: false,
        facts: Facts::OPAQUE,
    };
    let rewritten = inliner.transform_block(block);
    debug_assert!(inliner.values.is_empty(), "every inlined value reaches its read");
    rewritten
}

/// Each name that qualifies and the depth of its one read.
fn find_inline_candidates(block: &Block, version: LuaVersion) -> FxHashMap<CompactString, usize> {
    let mut scanner = CandidateScanner {
        declared_names: FxHashSet::default(),
        shadowed_names: FxHashSet::default(),
        references: FxHashMap::default(),
        open_declarations: FxHashSet::default(),
        opened_in_scope: Vec::new(),
        depth: 0,
        version,
    };
    scanner.visit_block(block);
    scanner
        .references
        .into_iter()
        .filter(|(name, reference)| {
            reference.count == 1
                && scanner.declared_names.contains(name)
                && !scanner.shadowed_names.contains(name)
        })
        .map(|(name, reference)| (name, reference.depth))
        .collect()
}

/// How often a name is read in its declaration's scope, and how deep the
/// first of those reads sits, which for a name read once is where its value
/// lands.
struct Reference {
    count: usize,
    depth: usize,
}

struct CandidateScanner {
    // Every single-name local with one initializer, so a second declaration
    // disqualifies the name.
    declared_names: FxHashSet<CompactString>,
    shadowed_names: FxHashSet<CompactString>,
    references: FxHashMap<CompactString, Reference>,
    /// Declared names whose scope the walk is inside.
    open_declarations: FxHashSet<CompactString>,
    /// `open_declarations` in opening order, to close them per scope.
    opened_in_scope: Vec<CompactString>,
    /// Levels of nesting open where the walk is.
    depth: usize,
    version: LuaVersion,
}

impl CandidateScanner {
    fn open_declaration(&mut self, name: &str) {
        self.open_declarations.insert(name.into());
        self.opened_in_scope.push(name.into());
    }

    fn close_scope(&mut self, scope_start: usize) {
        for name in self.opened_in_scope.drain(scope_start..) {
            self.open_declarations.remove(&name);
        }
    }

    fn shadow(&mut self, name: &str) {
        self.shadowed_names.insert(name.into());
    }

    /// A candidate is removed, so a write to its name would land on a global.
    fn shadow_write_target(&mut self, target: &Var) {
        if let Var::Name(name) = target {
            self.shadow(identifier_name(name));
        }
    }
}

impl<'ast> Visitor<'ast> for CandidateScanner {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        true
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_block(&mut self, block: &'ast Block) {
        let scope_start = self.opened_in_scope.len();
        self.walk_block(block);
        self.close_scope(scope_start);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                if has_fixed_binding(local, self.version) {
                    for attributed in local.names.iter() {
                        self.shadowed_names.insert(identifier_name(&attributed.name).into());
                    }
                    self.walk_stmt(stmt);
                    return;
                }
                if let Some(name) = single_declared_name(local) {
                    // The local's scope starts after its statement, so a
                    // read inside it is another binding's.
                    self.walk_stmt(stmt);
                    if self.declared_names.contains(name) {
                        self.shadowed_names.insert(name.into());
                    } else {
                        self.declared_names.insert(name.into());
                        self.open_declaration(name);
                    }
                    return;
                }
                // Bare and multi-name locals are binders too: a reference
                // between `local x` and a later `local x = 1` resolves to this
                // one, so the name can't inline.
                for attributed in local.names.iter() {
                    self.shadowed_names.insert(identifier_name(&attributed.name).into());
                }
            }
            Statement::LocalFunction(local_function) => {
                self.shadowed_names.insert(identifier_name(&local_function.name).into());
            }
            Statement::NumericFor(numeric_for) => {
                self.shadowed_names.insert(identifier_name(&numeric_for.name).into());
            }
            Statement::GenericFor(generic_for) => {
                for param in generic_for.names.iter() {
                    self.shadowed_names.insert(identifier_name(&param.name).into());
                }
            }
            Statement::RepeatLoop(repeat_loop) => {
                // `until` reads the body's locals, so its scope closes after it.
                let scope_start = self.opened_in_scope.len();
                self.walk_block(&repeat_loop.block);
                self.visit_expr(&repeat_loop.condition);
                self.close_scope(scope_start);
                return;
            }
            Statement::Assignment(assignment) => {
                for target in assignment.targets.iter() {
                    self.shadow_write_target(target);
                }
            }
            // Luau
            Statement::CompoundAssignment(compound) => self.shadow_write_target(&compound.var),
            // `function x()` writes `x`, and `function x.y()` indexes it.
            Statement::FunctionDecl(function_decl) => {
                if let Some(root) = function_decl.name.names.first() {
                    self.shadow(identifier_name(root));
                }
                if function_decl.name.method.is_some() {
                    self.shadow(METHOD_RECEIVER_NAME);
                }
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.shadow(identifier_name(&global_function.name));
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global) => {
                for attributed in global.names.iter() {
                    self.shadow(identifier_name(&attributed.name));
                }
            }
            // These bind no local. `walk_stmt` below still descends into
            // nested blocks.
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::IfStatement(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        for param in body.params.iter() {
            self.shadow(identifier_name(&param.name));
        }
        // Lua 5.5
        if let Some(vararg_name) = body.vararg.as_ref().and_then(|vararg| vararg.name.as_ref()) {
            self.shadow(identifier_name(vararg_name));
        }
        if has_implicit_arg(body, self.version) {
            self.shadow(IMPLICIT_VARARG_TABLE_NAME);
        }
        self.walk_function_body(body);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Name(token) = expr {
            let name = identifier_name(token);
            if !self.open_declarations.contains(name) {
                self.shadow(name);
            } else if let Some(reference) = self.references.get_mut(name) {
                reference.count = reference.count.saturating_add(1);
            } else {
                self.references.insert(name.into(), Reference { count: 1, depth: self.depth });
            }
        }
        self.walk_expr(expr);
    }
}

/// The one name of a `local` with one name and one initializer.
fn single_declared_name(local: &LocalAssignment) -> Option<&str> {
    let initializers = local.exprs.as_ref().map(|exprs| exprs.items.as_slice());
    if let ([attributed], Some([_])) = (local.names.items.as_slice(), initializers) {
        Some(identifier_name(&attributed.name))
    } else {
        None
    }
}

/// What an expression that may become a value evaluates to and how deep it
/// nests, gathered as the walk rebuilds it, so a value inlined into it is
/// never measured again.
#[derive(Clone, Copy)]
struct Facts {
    /// The kind of a literal, or of operators over literals, that can't raise,
    /// whose value and evaluation don't depend on where it runs. `None` for
    /// any other expression.
    kind: Option<LiteralKind>,
    /// The levels it nests once every run left unspliced is spliced.
    depth: usize,
    is_nonzero_number: bool,
    has_unspliced_run: bool,
}

impl Facts {
    const OPAQUE: Self =
        Self { kind: None, depth: 0, is_nonzero_number: false, has_unspliced_run: false };
}

/// A candidate's initializer, inlinable and waiting for the candidate's read.
struct Value {
    expr: Expression,
    facts: Facts,
}

struct Inliner {
    /// Each candidate's name and the depth of its one read.
    candidates: FxHashMap<CompactString, usize>,
    values: FxHashMap<CompactString, Value>,
    /// Whether the walk is rebuilding an expression that may become a value.
    /// There a value inlined bare into a run or a chain at its level stays
    /// unspliced until the value lands, so a chain of them splices once.
    is_in_value: bool,
    /// The facts of what `transform_expr` returned last, when `is_in_value`.
    facts: Facts,
}

impl Transform for Inliner {
    fn transform_block(&mut self, mut block: Block) -> Block {
        let stmts = mem::take(&mut block.stmts);
        block.stmts.reserve(stmts.len());
        for stmt in stmts {
            if let Statement::LocalAssignment(local) = stmt {
                block.stmts.extend(self.transform_local(local));
            } else {
                block.stmts.push(self.transform_stmt(stmt));
            }
        }
        if let Some(last) = &mut block.last_stmt {
            **last = self.transform_last_stmt(last.take());
        }
        block
    }

    /// A value inlined as the prefix gets parentheses unless it can stand
    /// bare there, as `x["k"]` with `x = 37.5` becomes `(37.5)["k"]`.
    fn transform_suffixed(&mut self, chain: Box<SuffixedExpression>) -> Box<SuffixedExpression> {
        let is_instantiation =
            chain.suffixes.iter().all(|suffix| matches!(suffix, Suffix::TypeInstantiation { .. }));
        if is_instantiation && self.is_in_value {
            return self.inline_instantiation(chain);
        }
        let was_in_value = mem::replace(&mut self.is_in_value, false);
        let mut walked = self.walk_suffixed(chain);
        self.is_in_value = was_in_value;
        if !is_prefix_expr(&walked.prefix) {
            walked.prefix = parenthesize(walked.prefix.take());
        }
        walked.splice_prefix();
        self.facts = Facts::OPAQUE;
        walked
    }

    fn transform_expr(&mut self, expr: Expression) -> Expression {
        match expr {
            Expression::Name(name) => {
                if let Some(value) = self.values.remove(identifier_name(&name)) {
                    return self.land(value);
                }
                self.facts = Facts::OPAQUE;
                Expression::Name(name)
            }
            literal @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)) => {
                self.facts = if self.is_in_value {
                    Facts {
                        kind: literal_kind(&literal),
                        depth: 0,
                        is_nonzero_number: is_nonzero_number_literal(&literal),
                        has_unspliced_run: false,
                    }
                } else {
                    Facts::OPAQUE
                };
                literal
            }
            Expression::Parenthesized(mut parenthesized) => {
                parenthesized.expr = self.transform_expr(parenthesized.expr);
                self.facts.depth = self.facts.depth.saturating_add(1);
                Expression::Parenthesized(parenthesized)
            }
            Expression::UnaryOp(mut unary_op) => {
                unary_op.operand = self.inline_operand(unary_op.operand, OperandPosition::Unary);
                let operand = self.facts;
                self.facts = Facts {
                    kind: operand.kind.and_then(|kind| unary_literal_kind(unary_op.op, kind)),
                    depth: operand.depth.saturating_add(1),
                    is_nonzero_number: false,
                    has_unspliced_run: operand.has_unspliced_run,
                };
                Expression::UnaryOp(unary_op)
            }
            Expression::BinaryOp(run) => self.inline_run(run),
            Expression::Suffixed(chain) => Expression::Suffixed(self.transform_suffixed(chain)),
            // Luau
            Expression::TypeCast(mut cast) => {
                cast.expr = self.inline_operand(cast.expr, OperandPosition::Cast);
                let operand = self.facts;
                let was_in_value = mem::replace(&mut self.is_in_value, false);
                cast.type_annotation = self.transform_type(cast.type_annotation);
                self.is_in_value = was_in_value;
                self.facts = if self.is_in_value {
                    let type_depth = type_nesting_depth(&cast.type_annotation);
                    Facts {
                        kind: operand.kind,
                        depth: operand.depth.max(type_depth).saturating_add(1),
                        is_nonzero_number: false,
                        has_unspliced_run: operand.has_unspliced_run,
                    }
                } else {
                    Facts::OPAQUE
                };
                Expression::TypeCast(cast)
            }
            other @ (Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_)) => {
                let was_in_value = mem::replace(&mut self.is_in_value, false);
                let walked = self.walk_expr(other);
                self.is_in_value = was_in_value;
                self.facts = Facts::OPAQUE;
                walked
            }
        }
    }
}

#[derive(Clone, Copy)]
enum OperandPosition {
    Left(BinOp),
    Right(BinOp),
    Unary,
    Cast,
}

/// Grouping lives in `Parenthesized` nodes and codegen prints the tree flat,
/// so a value that binds looser than its position would regroup on reparse.
///
/// A value is a closed literal expression, so none prints a Luau
/// if-expression last.
fn keeps_grouping(value: &Expression, position: OperandPosition) -> bool {
    match value {
        Expression::BinaryOp(run) => match position {
            OperandPosition::Left(op) => run.can_nest_bare(op, OperandSide::Left),
            OperandPosition::Right(op) => run.can_nest_bare(op, OperandSide::Right),
            OperandPosition::Unary => !binds_looser_than_unary(run),
            OperandPosition::Cast => false,
        },
        Expression::UnaryOp(_) => match position {
            // Unary ops bind looser than ^, so (-a)^b needs its parens.
            OperandPosition::Left(op) => op != BinOp::Pow,
            OperandPosition::Right(_) | OperandPosition::Unary => true,
            OperandPosition::Cast => false,
        },
        // Luau: `::` takes a simple expression, and `x :: A :: B` doesn't
        // parse.
        Expression::TypeCast(_) => !matches!(position, OperandPosition::Cast),
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => true,
    }
}

/// The operand `expr` prints last, which is what a following token lands on.
fn last_operand(expr: &mut Expression) -> &mut Expression {
    let mut current = expr;
    loop {
        current = match current {
            Expression::BinaryOp(binary_op) => &mut binary_op.links.last_mut().operand,
            Expression::UnaryOp(unop) => &mut unop.operand,
            Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::Nil(_)
            | Expression::True(_)
            | Expression::False(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => return current,
        };
    }
}

/// Splices every run and chain a value left unspliced in `expr`, which has
/// landed for good. Each splice takes in a whole nested run at once, so this
/// recurses no deeper than the spliced tree nests.
fn splice_unspliced(expr: &mut Expression) {
    match expr {
        Expression::BinaryOp(run) => {
            run.splice_operands();
            splice_unspliced(&mut run.first);
            for link in &mut run.links {
                splice_unspliced(&mut link.operand);
            }
        }
        Expression::Suffixed(chain) => {
            chain.splice_prefix();
            splice_unspliced(&mut chain.prefix);
        }
        Expression::UnaryOp(unop) => splice_unspliced(&mut unop.operand),
        Expression::Parenthesized(parenthesized) => splice_unspliced(&mut parenthesized.expr),
        Expression::TypeCast(cast) => splice_unspliced(&mut cast.expr),
        // A value holds no other node that leaves a run unspliced.
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => {}
    }
}

impl Inliner {
    /// Rewrites a `local`. A candidate's goes, and its value waits for the
    /// candidate's read, when the value is inlinable.
    fn transform_local(&mut self, mut local: Box<LocalAssignment>) -> Option<Statement> {
        let Some(read_depth) =
            single_declared_name(&local).and_then(|name| self.candidates.get(name).copied())
        else {
            return Some(self.walk_stmt(Statement::LocalAssignment(local)));
        };
        local.names = self.walk_attributed_names(local.names);
        if let (Some(exprs), [attributed]) = (&mut local.exprs, local.names.items.as_slice())
            && let [initializer] = exprs.items.as_mut_slice()
        {
            let was_in_value = mem::replace(&mut self.is_in_value, true);
            *initializer = self.transform_expr(initializer.take());
            self.is_in_value = was_in_value;
            let facts = self.facts;
            // The value can gain parentheses where it lands, and a cast inside
            // it a second pair.
            let is_inlinable = facts.kind.is_some()
                && facts.depth <= MAX_ADDED_DEPTH
                && read_depth.saturating_add(2).saturating_add(facts.depth) <= MAX_NESTING_DEPTH;
            if is_inlinable {
                let value = Value { expr: initializer.take(), facts };
                self.values.insert(identifier_name(&attributed.name).into(), value);
                return None;
            }
            if facts.has_unspliced_run {
                splice_unspliced(initializer);
            }
        }
        Some(Statement::LocalAssignment(local))
    }

    /// Hands back a value at its read. A value lands for good outside
    /// another value, so its unspliced runs splice there.
    fn land(&mut self, value: Value) -> Expression {
        let Value { mut expr, mut facts } = value;
        if !self.is_in_value && facts.has_unspliced_run {
            splice_unspliced(&mut expr);
            facts.has_unspliced_run = false;
        }
        self.facts = facts;
        expr
    }

    /// Only in an operand position can a value regroup, so only there does
    /// it gain parens. Anywhere else it stays bare, since `return (1+2)` is
    /// longer than `return 1+2` and `simplify_parens` wouldn't strip those
    /// parens.
    fn inline_operand(&mut self, operand: Expression, position: OperandPosition) -> Expression {
        if let Expression::Name(name) = &operand
            && let Some(value) = self.values.remove(identifier_name(name))
        {
            let expr = self.land(value);
            if keeps_grouping(&expr, position) {
                return expr;
            }
            self.facts.depth = self.facts.depth.saturating_add(1);
            return parenthesize(expr);
        }
        self.transform_expr(operand)
    }

    /// A value inlined bare where it would merge into its operator's run
    /// joins it, so a run of locals inlines without nesting.
    fn inline_run(&mut self, mut run: Box<BinaryOp>) -> Expression {
        // The operand before each `<` is checked while it's still a name.
        let mut before = &mut run.first;
        for link in &mut run.links {
            if link.op == BinOp::Lt {
                self.close_cast_before_less_than(before);
            }
            before = &mut link.operand;
        }
        let op = run.links.first().op;
        let inner_position = match inner_operand_side(&run) {
            OperandSide::Left => OperandPosition::Left(op),
            OperandSide::Right => OperandPosition::Right(op),
        };
        let mut operand_facts = Vec::new();
        if self.is_in_value {
            operand_facts.reserve(run.links.len().saturating_add(1));
        }
        run.first = self.inline_operand(run.first.take(), OperandPosition::Left(op));
        if self.is_in_value {
            operand_facts.push(self.facts);
        }
        let last_index = run.links.len().saturating_sub(1);
        for (i, link) in run.links.iter_mut().enumerate() {
            let position =
                if i == last_index { OperandPosition::Right(op) } else { inner_position };
            link.operand = self.inline_operand(link.operand.take(), position);
            if self.is_in_value {
                operand_facts.push(self.facts);
            }
        }
        if self.is_in_value {
            self.facts = run_facts(&run, &operand_facts);
        } else {
            run.splice_operands();
            self.facts = Facts::OPAQUE;
        }
        Expression::BinaryOp(run)
    }

    /// Luau: a chain of type instantiations, which evaluates to its prefix.
    fn inline_instantiation(
        &mut self,
        mut chain: Box<SuffixedExpression>,
    ) -> Box<SuffixedExpression> {
        chain.prefix = self.transform_expr(chain.prefix.take());
        let prefix = self.facts;
        let was_in_value = mem::replace(&mut self.is_in_value, false);
        let mut type_depth = 0usize;
        for suffix in &mut chain.suffixes {
            if let Suffix::TypeInstantiation { type_args, .. } = suffix {
                for type_arg in &mut type_args.args.items {
                    *type_arg = self.transform_type(type_arg.take());
                    type_depth = type_depth.max(type_nesting_depth(type_arg));
                }
            }
        }
        self.is_in_value = was_in_value;
        let is_joined = matches!(chain.prefix, Expression::Suffixed(_));
        let prefix_depth = if is_joined {
            // A chain that's the prefix joins this one once spliced.
            prefix.depth.saturating_sub(1)
        } else if is_prefix_expr(&chain.prefix) {
            prefix.depth
        } else {
            chain.prefix = parenthesize(chain.prefix.take());
            prefix.depth.saturating_add(1)
        };
        self.facts = Facts {
            kind: prefix.kind,
            depth: prefix_depth.max(type_depth).saturating_add(1),
            is_nonzero_number: false,
            has_unspliced_run: is_joined || prefix.has_unspliced_run,
        };
        chain
    }

    /// Luau: a cast has no closing token and `T<` opens generic arguments, so
    /// a cast inlined just before `<` needs parens of its own. A cast that was
    /// already there parsed fine, so only inlining can cause this.
    fn close_cast_before_less_than(&mut self, left: &mut Expression) {
        if let Expression::Name(name) = last_operand(left)
            && let Some(value) = self.values.get_mut(identifier_name(name))
        {
            let cast = last_operand(&mut value.expr);
            if matches!(cast, Expression::TypeCast(_)) {
                *cast = parenthesize(cast.take());
                splice_unspliced(&mut value.expr);
                value.facts.depth = expr_nesting_depth(&value.expr);
                value.facts.has_unspliced_run = false;
            }
        }
    }
}

/// The facts of `run` from those of its operands, first operand first.
///
/// Each operator applies in the order the level groups them, as
/// `literal_kind` folds a run. An operand that's a bare run at this level
/// nests no deeper here than its own operands, since splicing joins them.
fn run_facts(run: &BinaryOp, operands: &[Facts]) -> Facts {
    let level = run.precedence().0;
    let is_joined = |operand: &Expression| matches!(operand, Expression::BinaryOp(inner) if inner.precedence().0 == level);
    let operand_exprs = iter::once(&run.first).chain(run.links.iter().map(|link| &link.operand));
    let mut deepest = 0usize;
    let mut has_unspliced_run = false;
    for (operand, facts) in operand_exprs.zip(operands) {
        let is_operand_joined = is_joined(operand);
        has_unspliced_run |= is_operand_joined || facts.has_unspliced_run;
        let depth = if is_operand_joined { facts.depth.saturating_sub(1) } else { facts.depth };
        deepest = deepest.max(depth);
    }
    let kind = run_kind(run, operands);
    Facts { kind, depth: deepest.saturating_add(1), is_nonzero_number: false, has_unspliced_run }
}

/// What `run` evaluates to when its operands, first operand first, have
/// `operands`' kinds.
fn run_kind(run: &BinaryOp, operands: &[Facts]) -> Option<LiteralKind> {
    let kind_at = |i: usize| operands.get(i)?.kind;
    let is_nonzero_at = |i: usize| operands.get(i).is_some_and(|facts| facts.is_nonzero_number);
    match run.precedence().1 {
        Associativity::Left => {
            let mut kind = kind_at(0)?;
            for (i, link) in run.links.iter().enumerate() {
                let right = i.saturating_add(1);
                kind = binary_literal_kind(kind, link.op, kind_at(right)?, is_nonzero_at(right))?;
            }
            Some(kind)
        }
        Associativity::Right => {
            let mut kind = kind_at(run.links.len())?;
            for (i, link) in run.links.iter().enumerate().rev() {
                let is_nonzero = is_nonzero_at(i.saturating_add(1));
                kind = binary_literal_kind(kind_at(i)?, link.op, kind, is_nonzero)?;
            }
            Some(kind)
        }
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        run_pass(source, version, |block| inline(block, version))
    }

    #[test]
    fn inlines_single_use_number() {
        let result = apply(
            "\
local x = 42
return x
",
        );
        assert!(!result.contains("local"), "Declaration not removed: {result}");
        assert!(result.contains("42"), "Value not inlined: {result}");
    }

    #[test]
    fn no_inline_of_multi_use_local() {
        let result = apply(
            "\
local x = 42
print(x)
return x
",
        );
        assert!(result.contains("local"), "Multi-use local was inlined: {result}");
    }

    #[test]
    fn no_inline_of_call_initializer() {
        let result = apply(
            "\
local x = foo()
return x
",
        );
        assert!(result.contains("local"), "Side-effectful init was inlined: {result}");
    }

    #[test]
    fn no_inline_of_variable_read() {
        let result = apply(
            "\
local x = y
return x
",
        );
        assert!(result.contains("local"), "Variable read was inlined: {result}");
    }

    #[test]
    fn inlines_in_nested_function_body() {
        let result = apply(
            "\
local function foo()
  local x = 42
  return x
end
return foo
",
        );
        assert!(
            !result.contains("local x"),
            "Declaration inside function body not inlined: {result}"
        );
    }

    #[test]
    fn inlines_in_if_block() {
        let result = apply(
            "\
if true then
  local x = 42
  print(x)
end
",
        );
        assert!(!result.contains("local x"), "Declaration inside if block not inlined: {result}");
    }

    #[test]
    fn parenthesizes_literal_inlined_into_index_prefix() {
        let result = apply(
            "\
local x = 37.5
return x[\"k\"]
",
        );
        assert!(
            result.contains("(37.5)[\"k\"]"),
            "Literal in index-prefix position must gain parens: {result}"
        );
    }

    #[test]
    fn parenthesizes_literal_inlined_into_field_prefix() {
        let result = apply(
            "\
local x = \"s\"
return x.len
",
        );
        assert!(
            result.contains("(\"s\").len"),
            "Literal in field-prefix position must gain parens: {result}"
        );
    }

    fn apply_luau_reparsing(source: &str) -> String {
        let output = apply_version(source, LuaVersion::Luau);
        assert_reparses(&output, LuaVersion::Luau);
        output
    }

    #[test]
    fn parenthesizes_literal_inlined_into_type_instantiation() {
        for (source, expected) in [
            (
                "\
local s = \"x\"
return s<<T>>
",
                "return(\"x\")<<T>>",
            ),
            (
                "\
local s = 1
return s<<T>>
",
                "return(1)<<T>>",
            ),
            (
                "\
local s = nil
return s<<T>>
",
                "return(nil)<<T>>",
            ),
            (
                "\
local s = -1
return s<<T>>
",
                "return(-1)<<T>>",
            ),
            (
                "\
local s = \"x\" :: T
return s<<U>>
",
                "return(\"x\"::T)<<U>>",
            ),
            (
                "\
local s = \"x\"
return s<<T>><<U>>
",
                "return(\"x\")<<T>><<U>>",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn literal_inlined_under_instantiation_reparses_as_prefix() {
        apply_luau_reparsing(
            "\
local s = \"x\"
return s<<T>>()
",
        );
        apply_luau_reparsing(
            "\
local s = \"x\"
return s<<T>>.y
",
        );
        apply_luau_reparsing(
            "\
local s = \"x\"
s<<T>>()
",
        );
    }

    #[test]
    fn leaves_chained_type_instantiation_unwrapped() {
        // A chain is already a valid prefix, whether it sits on a var or an
        // inlined prefix.
        for (source, expected) in [
            (
                "\
local s = 1
print(s)
return f<<T>><<U>>
",
                "print(1)return f<<T>><<U>>",
            ),
            (
                "\
local s = (\"x\")<<T>>
return s<<U>>
",
                "return(\"x\")<<T>><<U>>",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn leaves_type_instantiation_prefixes_unwrapped() {
        for (source, expected) in [
            (
                "\
local s = 1
print(s)
return f<<T>>(x)
",
                "print(1)return f<<T>>(x)",
            ),
            (
                "\
local s = 1
print(s)
return f<<T>>.y
",
                "print(1)return f<<T>>.y",
            ),
            (
                "\
local s = 1
print(s)
f<<T>>(x)
",
                "print(1)f<<T>>(x)",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    fn assert_inlines(cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            let result = apply(source);
            assert_reparses(&result, LuaVersion::Lua54);
            assert_eq!(result, expected, "source: {source}");
        }
    }

    #[test]
    fn parenthesizes_inlined_operator_that_would_regroup() {
        assert_inlines(&[
            (
                "\
local s = 1 / 0
return 3 % s
",
                "return 3%(1/0)",
            ),
            (
                "\
local s = 1 + 2
return s * 2
",
                "return(1+2)*2",
            ),
            (
                "\
local s = 1 + 2
return 2 * s
",
                "return 2*(1+2)",
            ),
            (
                "\
local s = 1 - 2
return 5 - s
",
                "return 5-(1-2)",
            ),
            (
                "\
local s = 2 ^ 3
return s ^ 2
",
                "return(2^3)^2",
            ),
            (
                "\
local s = 1 .. 2
return s .. 3
",
                "return(1 ..2)..3",
            ),
            (
                "\
local s = 1 + 2
return 2 ^ s
",
                "return 2^(1+2)",
            ),
            (
                "\
local s = 1 | 2
return s & 3
",
                "return(1|2)&3",
            ),
            (
                "\
local s = 1 or 2
return s and 3
",
                "return(1 or 2)and 3",
            ),
            (
                "\
local s = -2
return s ^ y
",
                "return(-2)^y",
            ),
            (
                "\
local s = not nil
return s ^ y
",
                "return(not nil)^y",
            ),
        ]);
    }

    #[test]
    fn parenthesizes_operator_inlined_under_a_unary() {
        assert_inlines(&[
            (
                "\
local s = 1 + 2
return -s
",
                "return-(1+2)",
            ),
            (
                "\
local s = 1 == 2
return not s
",
                "return not(1==2)",
            ),
            (
                "\
local s = \"a\" .. \"b\"
return #s
",
                "return#(\"a\"..\"b\")",
            ),
            (
                "\
local s = 1 * 2
return ~s
",
                "return~(1*2)",
            ),
        ]);
    }

    #[test]
    fn parenthesizes_operator_inlined_into_a_nested_operand() {
        assert_inlines(&[
            (
                "\
local s = 1 + 2
return -s ^ 2
",
                "return-(1+2)^2",
            ),
            (
                "\
local s = -1
return -s ^ 2
",
                "return-(-1)^2",
            ),
            (
                "\
local s = 1 + 2
return (s * 3)
",
                "return((1+2)*3)",
            ),
            (
                "\
local s = 1 + 2
print(s * 3, 4)
",
                "print((1+2)*3,4)",
            ),
            (
                "\
local s = 1 + 2
return {s * 3}
",
                "return{(1+2)*3}",
            ),
            (
                "\
local s = 1 + 2
return t[s * 3]
",
                "return t[(1+2)*3]",
            ),
            (
                "\
local s = 1 + 2
if s * 3 then end
",
                "if(1+2)*3 then end",
            ),
            (
                "\
local a = 1 + 2
local b = a * 3
return b ^ 2
",
                "return((1+2)*3)^2",
            ),
        ]);
    }

    const BARE_INLINE_CASES: &[(&str, &str)] = &[
        (
            "\
local s = 1 + 2
return s
",
            "return 1+2",
        ),
        (
            "\
local s = 1 + 2
print(s)
",
            "print(1+2)",
        ),
        (
            "\
local s = 1 + 2
return {s}
",
            "return{1+2}",
        ),
        (
            "\
local s = 1 + 2
return t[s]
",
            "return t[1+2]",
        ),
        (
            "\
local s = 1 * 2
return s + 3
",
            "return 1*2+3",
        ),
        (
            "\
local s = 1 * 2
return 3 + s
",
            "return 3+1*2",
        ),
        (
            "\
local s = 1 - 2
return s - 5
",
            "return 1-2-5",
        ),
        (
            "\
local s = 2 ^ 3
return 2 ^ s
",
            "return 2^2^3",
        ),
        (
            "\
local s = 1 .. 2
return 3 .. s
",
            "return 3 ..1 ..2",
        ),
        (
            "\
local s = 1 + 2
return s < 4
",
            "return 1+2<4",
        ),
        (
            "\
local s = 1 < 2
return s == true
",
            "return 1<2==true",
        ),
        (
            "\
local s = 2 ^ 3
return -s
",
            "return-2^3",
        ),
        (
            "\
local s = -2
return 3 ^ s
",
            "return 3^-2",
        ),
        (
            "\
local s = -2
return s * 3
",
            "return-2*3",
        ),
        (
            "\
local s = -1
return -s
",
            "return- -1",
        ),
        (
            "\
local s = (1 + 2)
return s * 2
",
            "return(1+2)*2",
        ),
        (
            "\
local s = 2
return s * 3
",
            "return 2*3",
        ),
    ];

    #[test]
    fn leaves_inlined_expr_bare_where_its_grouping_holds() {
        assert_inlines(BARE_INLINE_CASES);
    }

    #[test]
    fn parenthesizes_expr_inlined_under_a_cast() {
        // `::` binds to a simple expression and doesn't chain.
        for (source, expected) in [
            (
                "\
local s = 1 + 2
return s :: any
",
                "return(1+2)::any",
            ),
            (
                "\
local s = -1
return s :: any
",
                "return(-1)::any",
            ),
            (
                "\
local s = 1 :: number
return s :: any
",
                "return(1::number)::any",
            ),
            (
                "\
local s = 1
return s :: any
",
                "return 1::any",
            ),
            (
                "\
local s = (1 + 2)
return s :: any
",
                "return(1+2)::any",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn parenthesizes_cast_inlined_before_less_than() {
        // `T<` opens generic arguments, so the cast has to end before the `<`.
        for (source, expected) in [
            (
                "\
local s = 1 :: number
return s < 2
",
                "return(1::number)<2",
            ),
            (
                "\
local s = 1 :: number
return 2 + s < 3
",
                "return 2+(1::number)<3",
            ),
            (
                "\
local s = 1 :: number
return -s < 2
",
                "return-(1::number)<2",
            ),
            (
                "\
local s = 1 :: number
return 1 < s < 3
",
                "return 1<(1::number)<3",
            ),
            (
                "\
local s = 2 + 1 :: number
return s < 3
",
                "return 2+(1::number)<3",
            ),
            (
                "\
local s = -1 :: number
return s < 3
",
                "return-(1::number)<3",
            ),
            (
                "\
local s = 2 + 1 :: number
return 2 * s < 3
",
                "return 2*(2+(1::number))<3",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn leaves_cast_bare_where_nothing_extends_its_type() {
        for (source, expected) in [
            (
                "\
local s = 1 :: number
return s
",
                "return 1::number",
            ),
            (
                "\
local s = 1 :: number
return s + 1
",
                "return 1::number+1",
            ),
            (
                "\
local s = 1 :: number
return 1 + s
",
                "return 1+1::number",
            ),
            (
                "\
local s = 1 :: number
return -s
",
                "return-1::number",
            ),
            (
                "\
local s = 1 :: number
return s > 2
",
                "return 1::number>2",
            ),
            (
                "\
local s = 1 :: number
return 2 < s
",
                "return 2<1::number",
            ),
            (
                "\
local s = 1
return x :: Foo<T> < s
",
                "return x::Foo<T><1",
            ),
            (
                "\
local s = (\"x\")<<T>>
return s .. \"y\"
",
                "return(\"x\")<<T>>..\"y\"",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn parenthesizes_literal_inlined_into_stmt_call_receiver() {
        let result = apply(
            "\
local x = false
x:resize(9)
",
        );
        assert!(
            result.contains("(false):resize(9)"),
            "Literal receiver of a statement call must gain parens: {result}"
        );
    }

    #[test]
    fn no_inline_of_table_constructor() {
        // Tables carry identity: moved into a loop, `{1,2,3}` would mint
        // a fresh table per iteration.
        let result = apply(
            "\
local t = {1, 2, 3}
return t
",
        );
        assert!(result.contains("local"), "Identity value was inlined: {result}");
    }

    #[test]
    fn no_inline_of_function_def() {
        // Closures carry identity and captures, and both depend on position:
        // in `local a=1 local f=function() return a end local a=2`, moving f's
        // body past the second `a` would rebind it.
        let result = apply(
            "\
local f = function() return 1 end
return f
",
        );
        assert!(result.contains("local"), "Closure was inlined: {result}");
    }

    #[test]
    fn inlines_chained_single_use() {
        // `x` is decided first, so `y`'s initializer is `42 + 1` by the time
        // `y` is, and both inline in the one walk.
        let result = apply(
            "\
local x = 42
local y = x + 1
return y
",
        );
        assert_eq!(result, "return 42+1");
    }

    #[test]
    fn inlines_a_chain_of_any_length_in_one_walk() {
        for links in [10u32, 100, 400, 20_000] {
            let mut sum = String::from("local x0 = 1\n");
            let mut concat = String::from("local x0 = \"z\"\n");
            for (i, previous) in (1..=links).zip(0u32..) {
                writeln!(sum, "local x{i} = x{previous} + 1").unwrap();
                writeln!(concat, "local x{i} = \"a\" .. x{previous}").unwrap();
            }
            writeln!(sum, "return x{links}").unwrap();
            writeln!(concat, "return x{links}").unwrap();
            let count = usize::try_from(links).expect("the link count fits");
            assert_eq!(apply(&sum), format!("return 1{}", "+1".repeat(count)));
            assert_eq!(apply(&concat), format!("return{}\"z\"", "\"a\"..".repeat(count)));
        }
    }

    #[test]
    fn stops_inlining_a_run_of_locals_before_it_nests_past_the_depth_cap() {
        // Each value needs parentheses where the next one uses it.
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..300u32).zip(0u32..) {
            let _unused = writeln!(source, "local x{i} = x{previous} * 2 + 1");
        }
        source.push_str("return x299\n");
        let result = apply(&source);
        assert_reparses(&result, LuaVersion::Lua54);
        assert!(result.contains("local"), "the run must stop inlining partway: {result}");
    }

    #[test]
    fn inlines_a_run_of_locals_that_joins_one_run_of_operators() {
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..300u32).zip(0u32..) {
            let _unused = writeln!(source, "local x{i} = x{previous} + 1");
        }
        source.push_str("return x299\n");
        let result = apply(&source);
        assert_reparses(&result, LuaVersion::Lua54);
        assert_eq!(result, format!("return 1{}", "+1".repeat(299)));
    }

    #[test]
    fn no_inline_of_vararg() {
        // Inside a nested function, `...` is that function's own varargs, or
        // a compile error if it takes none.
        let result = apply(
            "\
local function f(...)
  local x = ...
  return x
end
",
        );
        assert!(result.contains("local x"), "VarArg was inlined: {result}");
    }

    #[test]
    fn no_inline_of_multi_use_vararg() {
        let result = apply(
            "\
local function f(...)
  local x = ...
  print(x)
  return x
end
",
        );
        assert!(result.contains("local x"), "Multi-use vararg should not be inlined: {result}");
    }

    #[test]
    fn no_inline_of_variable_read_under_cast() {
        // TypeCast is transparent, and the inner variable read is impure.
        let result = apply_version(
            "\
local x = foo :: Bar
return x
",
            LuaVersion::Luau,
        );
        assert!(result.contains("local"), "TypeCast with var read should not be inlined: {result}");
    }

    #[test]
    fn no_inline_over_a_read_outside_the_declaration_scope() {
        for source in [
            "\
print(x)
local x = 1
",
            "\
local function g() return x end
local x = 1
print(g())
",
            "\
do local x = 1 end
print(x)
",
            "\
for i = 1, 1 do
  print(x)
  local x = 2
end
",
        ] {
            let result = apply(source);
            assert!(
                result.contains("print(x)") || result.contains("return x"),
                "{source}: {result}"
            );
        }
    }

    #[test]
    fn inlines_reads_inside_the_declaration_scope() {
        assert_inlines(&[
            (
                "\
local x = 1
print(x)
",
                "print(1)",
            ),
            (
                "\
local x = 1
local function f() return x end
",
                "local function f()return 1 end",
            ),
            (
                "\
if c then
  local x = 1
  print(x)
end
",
                "if c then print(1)end",
            ),
            (
                "\
do
  local x = 1
  do print(x) end
end
",
                "do do print(1)end end",
            ),
            (
                "\
repeat
  local x = 1
until x == y
",
                "repeat until 1==y",
            ),
            (
                "\
repeat
  local x = 1
  repeat until x
until z
",
                "repeat repeat until 1 until z",
            ),
        ]);
    }

    #[test]
    fn no_inline_over_a_write_to_the_name() {
        for source in [
            "\
local x = 1
x = 2
",
            "\
local x = 1
function x() end
print(x)
",
            "\
local x = \"s\"
function x.f() end
print(x)
",
        ] {
            let result = apply(source);
            assert!(result.contains("local x"), "{source}: {result}");
        }
        let compound = apply_version(
            "\
local x = 1
x += 2
",
            LuaVersion::Luau,
        );
        assert!(compound.contains("local x"), "{compound}");
    }

    #[test]
    fn no_inline_over_lua55_bindings() {
        for source in [
            "\
local args = 1
local function f(...args) return args end
",
            "\
local x = 1
global x
print(x)
",
            "\
local x = 1
global function x() end
print(x)
",
        ] {
            let result = apply_version(source, LuaVersion::Lua55);
            assert!(
                result.contains("local args") || result.contains("local x"),
                "{source}: {result}"
            );
        }
    }

    #[test]
    fn no_inline_across_global_function_shadow() {
        // The outer `x` is never read, and the `global function` body
        // declares its own `x` and reads it once. The scan has to see that
        // second declaration, or the outer value leaks in as
        // `global function g()return 1 end`.
        let result = apply_version(
            "\
local x = 1
global function g()
  local x = 2
  return x
end
return g
",
            LuaVersion::Lua55,
        );
        assert!(
            result.contains("local x=2")
                || result.contains("local x =2")
                || result.contains("return 2"),
            "Inner shadowed local must be preserved, not replaced by outer value: {result}"
        );
        assert!(
            !result.contains("return 1"),
            "Outer value must not leak into inner scope: {result}"
        );
    }

    #[test]
    fn no_inline_into_a_method_that_binds_self() {
        let result = apply(
            "\
local self = 5
function M:get() return self end
",
        );
        assert!(
            result.contains("return self"),
            "`self` in a `:` method is the receiver, not the outer local: {result}"
        );
    }

    #[test]
    fn no_inline_into_a_lua51_vararg_function_that_binds_arg() {
        let result = apply_version(
            "\
local arg = 5
local function f(...) return arg end
return f
",
            LuaVersion::Lua51,
        );
        assert!(
            result.contains("return arg"),
            "`arg` in a 5.1 vararg function is its implicit table, not the outer local: {result}"
        );
    }

    #[test]
    fn no_inline_of_a_literal_operation_that_raises() {
        let result = apply(
            "\
local x = 1 + nil
print(\"before\")
return x
",
        );
        assert!(
            !result.starts_with("print"),
            "the error must still fire before the print runs: {result}"
        );
    }
}
