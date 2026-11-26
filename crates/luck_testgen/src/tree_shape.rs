//! A tree printed without what a printer may change.
//!
//! That's spans, trailing separators in tables, a union's or intersection's
//! leading operator, a literal's spelling, whether a lone string or table
//! argument has parentheses, where a run of operators that group the same
//! either way splits, and the parentheses the tree keeps where they change
//! nothing. It keeps them only to truncate a call or `...` to one value,
//! which changes something only at the end of a list.

use luck_ast::{
    BinaryLink, BinaryOp, Block, ConditionalBinding, Declaration, DeclaredFunction, Expression,
    ExternTypeMember, Field, FunctionArgs, FunctionAttribute, FunctionBody, FunctionName,
    FunctionTypeParam, GenericTypeList, GenericTypeParam, IfStatement, Name, Statement,
    StringLiteral, Suffix, SuffixedExpression, TableConstructor, Type, TypeArgs, TypeDeclaration,
    TypeDeclarationValue, TypeField, TypedName, Var, VarArgParam,
};
use luck_token::Associativity;

/// `chunk` as one node per line, its children indented below it.
///
/// Two trees print the same when a printer may turn one into the other, so
/// comparing a tree with the parse of its printed output checks that the
/// printer kept its meaning.
pub fn tree_shape(chunk: &Block<'_>) -> String {
    let mut printer = Printer { output: String::new(), depth: 0 };
    printer.block(chunk);
    printer.output
}

struct Printer {
    output: String,
    depth: usize,
}

impl Printer {
    fn line(&mut self, text: &str) {
        for _ in 0..self.depth {
            self.output.push_str("  ");
        }
        self.output.push_str(text);
        self.output.push('\n');
    }

    /// Prints `text` and indents what follows, until [`Self::close`].
    fn open(&mut self, text: &str) {
        self.line(text);
        self.depth += 1;
    }

    const fn close(&mut self) {
        self.depth -= 1;
    }

    fn string(literal: &StringLiteral<'_>) -> String {
        format!("{:?}", literal.value)
    }

    fn block(&mut self, block: &Block<'_>) {
        self.open("Block");
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        self.close();
    }

    fn stmt(&mut self, stmt: &Statement<'_>) {
        match stmt {
            Statement::Assignment(assignment) => {
                self.open("Assignment");
                for target in &assignment.targets {
                    self.var(target);
                }
                self.exprs("values", &assignment.values);
                self.close();
            }
            Statement::FunctionCall(call) => self.suffixed("FunctionCall", call),
            Statement::DoBlock(do_block) => {
                self.open("Do");
                self.block(&do_block.block);
                self.close();
            }
            Statement::WhileLoop(while_loop) => {
                self.open("While");
                self.binding(while_loop.binding.as_deref());
                self.expr(&while_loop.condition);
                self.block(&while_loop.block);
                self.close();
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.open("Repeat");
                self.block(&repeat_loop.block);
                self.expr(&repeat_loop.condition);
                self.close();
            }
            Statement::IfStatement(if_statement) => self.if_statement(if_statement),
            Statement::NumericFor(numeric_for) => {
                self.open("NumericFor");
                self.typed_name(&numeric_for.variable);
                self.expr(&numeric_for.start);
                self.expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.expr(step);
                }
                self.block(&numeric_for.block);
                self.close();
            }
            Statement::GenericFor(generic_for) => {
                self.open("GenericFor");
                for name in &generic_for.names {
                    self.typed_name(name);
                }
                self.exprs("in", &generic_for.exprs);
                self.block(&generic_for.block);
                self.close();
            }
            Statement::FunctionDecl(decl) => {
                self.open("FunctionDecl");
                self.attributes(&decl.attributes);
                self.function_name(&decl.name);
                self.function_body(&decl.body);
                self.close();
            }
            Statement::LocalFunction(function) => {
                self.open(&format!("LocalFunction {:?} {}", function.kind, function.name.text));
                self.attributes(&function.attributes);
                self.function_body(&function.body);
                self.close();
            }
            Statement::LocalAssignment(assignment) => {
                self.open(&format!("LocalAssignment {:?}", assignment.kind));
                for name in &assignment.names {
                    self.typed_name(name);
                }
                self.exprs("values", &assignment.exprs);
                self.close();
            }
            Statement::Return(return_stmt) => self.exprs("Return", &return_stmt.exprs),
            Statement::Break(_) => self.line("Break"),
            Statement::Continue(_) => self.line("Continue"),
            Statement::CompoundAssignment(assignment) => {
                self.open(&format!("CompoundAssignment {:?}", assignment.op));
                self.var(&assignment.var);
                self.expr(&assignment.expr);
                self.close();
            }
            Statement::TypeDeclaration(declaration) => self.type_declaration(declaration),
            Statement::Declaration(declaration) => self.declaration(declaration),
            Statement::Error(_) => self.line("Error"),
        }
    }

    fn declaration(&mut self, declaration: &Declaration<'_>) {
        match declaration {
            Declaration::Global(global) => {
                self.open(&format!("DeclaredGlobal {}", global.name.text));
                self.ty(&global.ty);
                self.close();
            }
            Declaration::Function(function) => self.declared_function(function),
            Declaration::ExternType(extern_type) => {
                let supertype = extern_type
                    .supertype
                    .map_or_else(String::new, |supertype| format!(" extends {}", supertype.text));
                self.open(&format!("DeclaredExternType {}{supertype}", extern_type.name.text));
                for member in &extern_type.members {
                    match member {
                        ExternTypeMember::Field(field) => self.type_field(field),
                        ExternTypeMember::Method(method) => self.declared_function(method),
                    }
                }
                self.close();
            }
        }
    }

    fn declared_function(&mut self, function: &DeclaredFunction<'_>) {
        self.open(&format!("DeclaredFunction {}", function.name.text));
        self.attributes(&function.attributes);
        if let Some(chain) = &function.declare_chain {
            self.suffixed("DeclareChain", chain);
        }
        self.generic_type_list(function.generics.as_deref());
        self.params(&function.params, function.vararg.as_deref());
        if let Some(return_type) = &function.return_type {
            self.open("returns");
            self.ty(return_type);
            self.close();
        }
        self.close();
    }

    fn if_statement(&mut self, if_statement: &IfStatement<'_>) {
        self.open("If");
        self.binding(if_statement.binding.as_deref());
        self.expr(&if_statement.condition);
        self.block(&if_statement.block);
        for clause in &if_statement.elseif_clauses {
            self.open("ElseIf");
            self.binding(clause.binding.as_deref());
            self.expr(&clause.condition);
            self.block(&clause.block);
            self.close();
        }
        if let Some(clause) = &if_statement.else_clause {
            self.open("Else");
            self.block(&clause.block);
            self.close();
        }
        self.close();
    }

    fn type_declaration(&mut self, declaration: &TypeDeclaration<'_>) {
        let exported = if declaration.is_exported { "export " } else { "" };
        self.open(&format!("TypeDeclaration {exported}{}", declaration.name.text));
        self.generic_type_list(declaration.generics.as_deref());
        match &declaration.value {
            TypeDeclarationValue::Alias(ty) => self.ty(ty),
            TypeDeclarationValue::TypeFunction(body) => self.function_body(body),
        }
        self.close();
    }

    fn exprs(&mut self, label: &str, exprs: &[Expression<'_>]) {
        self.open(label);
        self.list(exprs);
        self.close();
    }

    /// The last expression of a list yields all its values, so only there
    /// does a parenthesized call or `...` differ from a bare one.
    fn list(&mut self, exprs: &[Expression<'_>]) {
        if let Some((last, rest)) = exprs.split_last() {
            for expr in rest {
                self.expr(expr);
            }
            self.value(last, true);
        }
    }

    fn expr(&mut self, expr: &Expression<'_>) {
        self.value(expr, false);
    }

    fn value(&mut self, expr: &Expression<'_>, is_expanded: bool) {
        match expr {
            Expression::Nil(_) => self.line("Nil"),
            Expression::False(_) => self.line("False"),
            Expression::True(_) => self.line("True"),
            Expression::Number(literal) => self.line(&format!("Number {:?}", literal.value)),
            Expression::StringLiteral(literal) => {
                self.line(&format!("String {}", Self::string(literal)));
            }
            Expression::VarArg(_) => self.line("VarArg"),
            Expression::FunctionDef(def) => {
                self.open("FunctionDef");
                self.attributes(&def.attributes);
                self.function_body(&def.body);
                self.close();
            }
            Expression::Name(name) => self.name("Name", name),
            Expression::Suffixed(chain) => self.suffixed("Suffixed", chain),
            Expression::Parenthesized(parenthesized) => {
                let inner = parenthesized.expr.unwrap_parentheses();
                if is_expanded && inner.is_multi_value() {
                    self.open("OneValue");
                    self.expr(inner);
                    self.close();
                } else {
                    self.expr(inner);
                }
            }
            Expression::TableConstructor(table) => self.table(table),
            Expression::BinaryOp(binary) => self.binary(binary),
            Expression::UnaryOp(unary) => {
                self.open(&format!("UnaryOp {:?}", unary.op));
                self.expr(&unary.operand);
                self.close();
            }
            Expression::IfExpression(if_expr) => {
                self.open("IfExpression");
                self.binding(if_expr.binding.as_deref());
                self.expr(&if_expr.condition);
                self.expr(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    self.open("ElseIf");
                    self.binding(clause.binding.as_deref());
                    self.expr(&clause.condition);
                    self.expr(&clause.expr);
                    self.close();
                }
                self.expr(&if_expr.else_expr);
                self.close();
            }
            Expression::InterpolatedString(interpolated) => {
                self.open("InterpolatedString");
                for segment in &interpolated.segments {
                    self.line(&format!("text {}", Self::string(&segment.text)));
                    self.expr(&segment.expr);
                }
                self.line(&format!("text {}", Self::string(&interpolated.tail)));
                self.close();
            }
            Expression::TypeCast(cast) => {
                self.open("TypeCast");
                self.expr(&cast.expr);
                self.ty(&cast.ty);
                self.close();
            }
            Expression::Error(_) => self.line("Error"),
        }
    }

    fn name(&mut self, kind: &str, name: &Name<'_>) {
        self.line(&format!("{kind} {}", name.text));
    }

    fn var(&mut self, var: &Var<'_>) {
        match var {
            Var::Name(name) => self.name("Name", name),
            Var::Suffixed(chain) => self.suffixed("Suffixed", chain),
        }
    }

    fn suffixed(&mut self, kind: &str, chain: &SuffixedExpression<'_>) {
        self.open(kind);
        self.expr(&chain.prefix);
        for suffix in &chain.suffixes {
            self.suffix(suffix);
        }
        self.close();
    }

    fn suffix(&mut self, suffix: &Suffix<'_>) {
        match suffix {
            Suffix::Field(name) => self.name("Field", name),
            Suffix::Index { index, .. } => {
                self.open("Index");
                self.expr(index);
                self.close();
            }
            Suffix::Call(args) => self.args("Call", args),
            Suffix::MethodCall(call) => {
                self.open(&format!("MethodCall {}", call.name.text));
                if let Some(type_args) = &call.type_args {
                    self.type_args(type_args);
                }
                self.args("args", &call.args);
                self.close();
            }
            Suffix::TypeInstantiation { type_args, .. } => {
                self.open("TypeInstantiation");
                self.type_args(type_args);
                self.close();
            }
        }
    }

    /// `f "s"` and `f {}` are `f("s")` and `f({})`.
    fn args(&mut self, label: &str, args: &FunctionArgs<'_>) {
        self.open(label);
        match args {
            FunctionArgs::Parenthesized { args: exprs, .. } => self.list(exprs),
            FunctionArgs::TableConstructor(table) => self.table(table),
            FunctionArgs::StringLiteral(literal) => {
                self.line(&format!("String {}", Self::string(literal)));
            }
            FunctionArgs::Error(_) => self.line("Error"),
        }
        self.close();
    }

    /// A run's operators share one precedence, so its associativity groups
    /// it: `a - b - c` is `(a - b) - c`, and `a .. b .. c` is `a .. (b .. c)`.
    /// An operand that's a run at the same level, where it groups as the run
    /// would without it, joins the run, so `(a - b) - c` prints as
    /// `a - b - c`. Each operator holds the operand after it, so a run nests
    /// no deeper however long it grows.
    fn binary(&mut self, binary: &BinaryOp<'_>) {
        self.open("BinaryOp");
        match binary.precedence().1 {
            Associativity::Left => self.left_run(binary),
            Associativity::Right => {
                self.expr(&binary.first);
                self.right_links(binary);
            }
        }
        self.close();
    }

    /// The operands and operators of `run`, a left-associative one.
    fn left_run(&mut self, run: &BinaryOp<'_>) {
        match joining_run(&run.first, run) {
            Some(first) => self.left_run(first),
            None => self.expr(&run.first),
        }
        self.links(&run.links);
    }

    /// The operators of `run`, a right-associative one, and the operands
    /// after them.
    fn right_links(&mut self, run: &BinaryOp<'_>) {
        let last = run.links.last();
        self.links(&run.links[..run.links.len() - 1]);
        let joining = joining_run(&last.operand, run);
        self.open(&format!("{:?}", last.op));
        self.expr(joining.map_or(&last.operand, |last_run| &last_run.first));
        self.close();
        if let Some(last_run) = joining {
            self.right_links(last_run);
        }
    }

    fn links(&mut self, links: &[BinaryLink<'_>]) {
        for link in links {
            self.open(&format!("{:?}", link.op));
            self.expr(&link.operand);
            self.close();
        }
    }

    fn table(&mut self, table: &TableConstructor<'_>) {
        self.open("Table");
        let last = table.fields.len().saturating_sub(1);
        for (i, field) in table.fields.iter().enumerate() {
            match field {
                Field::Bracketed { key, value, .. } => {
                    self.open("Bracketed");
                    self.expr(key);
                    self.expr(value);
                }
                Field::Named { name, value } => {
                    self.open(&format!("Named {}", name.text));
                    self.expr(value);
                }
                Field::Positional(value) => {
                    self.open("Positional");
                    self.value(value, i == last);
                }
            }
            self.close();
        }
        self.close();
    }

    fn function_name(&mut self, name: &FunctionName<'_>) {
        let path: Vec<&str> = name.names.iter().map(|part| part.text.as_str()).collect();
        let method = name.method.map_or_else(String::new, |method| format!(":{}", method.text));
        self.line(&format!("name {}{method}", path.join(".")));
    }

    fn function_body(&mut self, body: &FunctionBody<'_>) {
        self.open("FunctionBody");
        self.generic_type_list(body.generics.as_deref());
        self.params(&body.params, body.vararg.as_deref());
        if let Some(return_type) = &body.return_type {
            self.open("returns");
            self.ty(return_type);
            self.close();
        }
        self.block(&body.block);
        self.close();
    }

    fn params(&mut self, params: &[TypedName<'_>], vararg: Option<&VarArgParam<'_>>) {
        for param in params {
            self.typed_name(param);
        }
        if let Some(vararg) = vararg {
            self.open("VarArgParam");
            self.optional_type(vararg.type_annotation.as_deref());
            self.close();
        }
    }

    fn binding(&mut self, binding: Option<&ConditionalBinding<'_>>) {
        if let Some(binding) = binding {
            self.open(if binding.is_const { "Binding const" } else { "Binding local" });
            self.typed_name(&binding.name);
            self.close();
        }
    }

    fn typed_name(&mut self, typed_name: &TypedName<'_>) {
        self.open(&format!("TypedName {}", typed_name.name.text));
        self.optional_type(typed_name.type_annotation.as_deref());
        self.close();
    }

    fn attributes(&mut self, attributes: &[FunctionAttribute<'_>]) {
        for attribute in attributes {
            self.open(&format!("FunctionAttribute {}", attribute.name.text));
            if let Some(args) = &attribute.args {
                self.exprs("args", args);
            }
            self.close();
        }
    }

    fn optional_type(&mut self, annotation: Option<&Type<'_>>) {
        if let Some(ty) = annotation {
            self.open("type");
            self.ty(ty);
            self.close();
        }
    }

    fn type_args(&mut self, type_args: &TypeArgs<'_>) {
        self.types("TypeArgs", &type_args.args);
    }

    fn types(&mut self, label: &str, types: &[Type<'_>]) {
        self.open(label);
        for ty in types {
            self.ty(ty);
        }
        self.close();
    }

    fn generic_type_list(&mut self, list: Option<&GenericTypeList<'_>>) {
        let Some(GenericTypeList { params, .. }) = list else {
            return;
        };
        self.open("Generics");
        for GenericTypeParam { name, is_pack, default, .. } in params {
            let pack = if *is_pack { "..." } else { "" };
            self.open(&format!("GenericTypeParam {}{pack}", name.text));
            self.optional_type(default.as_deref());
            self.close();
        }
        self.close();
    }

    fn ty(&mut self, ty: &Type<'_>) {
        match ty {
            Type::Name(name) => self.name("TypeName", name),
            Type::Named(named) => {
                let prefix =
                    named.prefix.map_or_else(String::new, |prefix| format!("{}.", prefix.text));
                self.open(&format!("Named {prefix}{}", named.name.text));
                if let Some(generics) = &named.generics {
                    self.type_args(generics);
                }
                self.close();
            }
            Type::Typeof(typeof_type) => {
                self.open("Typeof");
                self.expr(&typeof_type.expr);
                self.close();
            }
            Type::Table(table) => {
                self.open("TableType");
                for field in &table.fields {
                    self.type_field(field);
                }
                self.close();
            }
            Type::Function(function) => {
                self.open("FunctionType");
                self.attributes(&function.attributes);
                self.generic_type_list(function.generics.as_deref());
                for FunctionTypeParam { name, ty: param_ty } in &function.params {
                    let label = name.map_or_else(String::new, |param| format!(" {}", param.text));
                    self.open(&format!("param{label}"));
                    self.ty(param_ty);
                    self.close();
                }
                self.open("returns");
                self.ty(&function.return_type);
                self.close();
                self.close();
            }
            Type::Optional(optional) => {
                self.open(&format!("Optional {}", optional.question_count));
                self.ty(&optional.ty);
                self.close();
            }
            // `| T` and `& T` are `T`.
            Type::Union(union) => {
                if let [member] = &*union.types {
                    self.ty(member);
                } else {
                    self.open("Union");
                    self.union_members(&union.types);
                    self.close();
                }
            }
            Type::Intersection(intersection) => {
                if let [member] = &*intersection.types {
                    self.ty(member);
                } else {
                    self.open("Intersection");
                    self.intersection_members(&intersection.types);
                    self.close();
                }
            }
            Type::Parenthesized(parenthesized) if parenthesized.attributes.is_empty() => {
                self.ty(&parenthesized.ty);
            }
            Type::Parenthesized(parenthesized) => {
                self.open("AttributedParentheses");
                self.attributes(&parenthesized.attributes);
                self.ty(&parenthesized.ty);
                self.close();
            }
            Type::Pack(pack) => self.types("TypePack", &pack.types),
            Type::Nil(_) => self.line("NilType"),
            Type::True(_) => self.line("TrueType"),
            Type::False(_) => self.line("FalseType"),
            Type::StringLiteral(literal) => {
                self.line(&format!("StringType {}", Self::string(literal)));
            }
            Type::Variadic(variadic) => {
                self.open("Variadic");
                self.ty(&variadic.ty);
                self.close();
            }
            Type::GenericPack(pack) => self.line(&format!("GenericPack {}", pack.name.text)),
            Type::Error(_) => self.line("Error"),
        }
    }

    /// A union in a union, parenthesized or not, adds its members to it.
    fn union_members(&mut self, members: &[Type<'_>]) {
        for member in members {
            if let Type::Union(inner) = without_parentheses(member) {
                self.union_members(&inner.types);
            } else {
                self.ty(member);
            }
        }
    }

    /// As [`Self::union_members`], for an intersection.
    fn intersection_members(&mut self, members: &[Type<'_>]) {
        for member in members {
            if let Type::Intersection(inner) = without_parentheses(member) {
                self.intersection_members(&inner.types);
            } else {
                self.ty(member);
            }
        }
    }

    fn type_field(&mut self, field: &TypeField<'_>) {
        match field {
            TypeField::Named { access, name, value, .. } => {
                self.open(&format!("Named {access:?} {}", name.text));
                self.ty(value);
            }
            TypeField::Indexer { access, key, value, .. } => {
                self.open(&format!("Indexer {access:?}"));
                self.ty(key);
                self.ty(value);
            }
            TypeField::Array { access, value, .. } => {
                self.open(&format!("Array {access:?}"));
                self.ty(value);
            }
        }
        self.close();
    }
}

/// `operand`, of `run`, when it's a run at `run`'s level, which parentheses
/// around it don't change.
fn joining_run<'tree, 'ast>(
    operand: &'tree Expression<'ast>,
    run: &BinaryOp<'_>,
) -> Option<&'tree BinaryOp<'ast>> {
    if let Expression::BinaryOp(inner) = operand.unwrap_parentheses()
        && inner.precedence().0 == run.precedence().0
    {
        Some(inner)
    } else {
        None
    }
}

fn without_parentheses<'tree, 'ast>(ty: &'tree Type<'ast>) -> &'tree Type<'ast> {
    let mut current = ty;
    while let Type::Parenthesized(parenthesized) = current
        && parenthesized.attributes.is_empty()
    {
        current = &parenthesized.ty;
    }
    current
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;

    use super::*;

    fn shape(source: &str) -> String {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes());
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        tree_shape(&parsed.chunk)
    }

    #[test]
    fn what_a_printer_may_change_prints_the_same() {
        assert_eq!(
            shape("f 'a\\x41' ; local t = {1, 2,}\ntype T = | A | B x = 0XFF"),
            shape("f(\"aA\") local t = {1, 2} type T = A | B x = 0xff")
        );
        assert_eq!(
            shape("x = (-(3)) + ((a - b) - c) local y = (f()), g() type U = (A | (B | C))"),
            shape("x = -3 + (a - b - c) local y = f(), g() type U = A | B | C")
        );
        assert_eq!(
            shape("x = ((a - b) + c) - d y = a .. (b .. (c .. d)) z = 2 ^ (3 ^ 4)"),
            shape("x = a - b + c - d y = a .. b .. c .. d z = 2 ^ 3 ^ 4")
        );
    }

    #[test]
    fn a_long_run_nests_no_deeper_than_a_short_one() {
        let links = 2_000;
        for op in ["+", ".."] {
            let source = format!("return x{}", format!(" {op} x").repeat(links));
            let longest_line = shape(&source).lines().map(str::len).max();
            assert!(longest_line.is_some_and(|width| width < 20), "{op}: {longest_line:?}");
        }
    }

    #[test]
    fn a_change_in_meaning_prints_differently() {
        assert_ne!(shape("x = (a or b) and c"), shape("x = a or b and c"));
        assert_ne!(shape("x = a - b - c"), shape("x = a - (b - c)"));
        assert_ne!(shape("f(g())"), shape("f((g()))"));
        assert_ne!(shape("return {f()}"), shape("return {(f())}"));
        assert_ne!(shape("x = a .. b .. c"), shape("x = (a .. b) .. c"));
        assert_ne!(shape("x = a - b - c - d"), shape("x = a - (b - c) - d"));
        assert_ne!(shape("x = a .. b .. c .. d"), shape("x = a .. (b .. c) .. d"));
        assert_ne!(shape("x = a - b + c"), shape("x = a + b - c"));
        assert_ne!(shape(r#"x = "\xFE""#), shape(r#"x = "\xFF""#));
    }
}
