use super::{FullGenerator, Varargs};

const MAX_STMT_DEPTH: usize = 4;
const MAX_EXPR_DEPTH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopEnd {
    End,
    Until,
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the generator is one type with its grammar split by file"
)]
impl FullGenerator {
    pub(super) fn stmt(&mut self) {
        let can_nest = self.stmt_depth < MAX_STMT_DEPTH;
        match self.rng.below(24) {
            20 if self.version.is_luau() => self.type_alias_stmt(),
            20 if self.version.has_global() => self.line("global sink"),
            20 if self.version.has_empty_stmt() => self.line(";"),
            0..=4 | 20 => self.local_declaration(),
            10 if self.version.has_compound_assignment() => self.compound_assignment(),
            8..=10 => self.assignment(),
            11..=12 if can_nest => self.if_stmt(),
            13 if can_nest => self.numeric_for(),
            14 if can_nest => self.generic_for(),
            15 if can_nest => self.while_stmt(),
            16 if can_nest => self.repeat_stmt(),
            17..=18 if can_nest => self.function_stmt(),
            19 if can_nest => self.do_block(),
            21 => self.multi_local(),
            22 => self.multi_assignment(),
            _ => self.call_stmt(),
        }
    }

    fn local_declaration(&mut self) {
        let name = self.fresh_name();
        let value = self.expr(MAX_EXPR_DEPTH);
        let semicolon = if self.rng.chance(15) { ";" } else { "" };
        if self.version.has_attributes() && self.rng.chance(10) {
            self.readonly_names.push(name.clone());
            if self.version.has_leading_attributes() && self.rng.chance(50) {
                // Lua 5.5
                self.line(&format!("local <const> {name} = {value}{semicolon}"));
            } else {
                // Lua 5.4+
                self.line(&format!("local {name} <const> = {value}{semicolon}"));
            }
        } else if self.version.is_luau() && self.rng.chance(10) {
            // Luau const binding
            self.readonly_names.push(name.clone());
            self.line(&format!("const {name} = {value}{semicolon}"));
        } else if self.version.is_luau() && self.rng.chance(35) {
            let annotation = self.type_expr(2);
            self.line(&format!("local {name}: {annotation} = {value}{semicolon}"));
        } else {
            self.line(&format!("local {name} = {value}{semicolon}"));
        }
    }

    fn multi_local(&mut self) {
        let first = self.fresh_name();
        let second = self.fresh_name();
        let callee = self.known_callable();
        let arg = self.expr(1);
        self.line(&format!("local {first}, {second} = {callee}({arg})"));
    }

    fn assignment(&mut self) {
        let target = self.assign_target();
        let value = self.expr(MAX_EXPR_DEPTH);
        self.line(&format!("{target} = {value}"));
    }

    fn multi_assignment(&mut self) {
        let first = self.assign_target();
        let second = self.assign_target();
        let value_a = self.expr(1);
        let value_b = self.expr(1);
        self.line(&format!("{first}, {second} = {value_b}, {value_a}"));
    }

    fn assign_target(&mut self) -> String {
        let base = self.known_writable_name();
        match self.rng.below(4) {
            0 => base,
            1 => format!("{base}.field"),
            2 => {
                // Spaces keep a long-bracket string key from forming `[[`.
                let key = self.expr(1);
                format!("{base}[ {key} ]")
            }
            _ => format!("{base}.nested.slot"),
        }
    }

    fn compound_assignment(&mut self) {
        // Luau
        let target = self.assign_target();
        let op = self.rng.pick(&["+=", "-=", "*=", "/=", "//=", "%=", "^=", "..="]);
        let value = self.expr(2);
        self.line(&format!("{target} {op} {value}"));
    }

    fn call_stmt(&mut self) {
        let callee = self.known_callable();
        match self.rng.below(6) {
            0 => {
                let receiver = self.known_name();
                let method = self.rng.pick(&["update", "resize", "render", "insert"]);
                let arg = self.expr(2);
                self.line(&format!("{receiver}:{method}({arg})"));
            }
            1 => self.line(&format!("{callee} \"literal argument\"")),
            2 => {
                let field = self.expr(1);
                self.line(&format!("{callee} {{ tag = \"item\", {field} }}"));
            }
            3 => {
                let first = self.expr(2);
                let second = self.expr(1);
                self.line(&format!("{callee}({first}, {second})"));
            }
            4 => self.line(&format!("{callee} [[long-bracket argument]]")),
            _ => {
                let arg = self.expr(2);
                self.line(&format!("{callee}({arg})"));
            }
        }
    }

    fn if_stmt(&mut self) {
        let condition = self.condition();
        self.line(&format!("if {condition} then"));
        self.block(|generator| generator.small_body(2));
        if self.rng.chance(40) {
            let elseif_condition = self.condition();
            self.line(&format!("elseif {elseif_condition} then"));
            self.block(|generator| generator.small_body(1));
        }
        if self.rng.chance(45) {
            self.line("else");
            self.block(|generator| generator.small_body(1));
        }
        self.line("end");
    }

    fn numeric_for(&mut self) {
        let loop_variable = self.fresh_name();
        // 5.5 makes for control variables read-only.
        if self.version.has_const_for_variables() {
            self.readonly_names.push(loop_variable.clone());
        }
        let stop = self.expr(1);
        let header = if self.rng.chance(40) {
            format!("for {loop_variable} = 1, {stop}, 2 do")
        } else {
            format!("for {loop_variable} = 1, {stop} do")
        };
        self.line(&header);
        self.loop_body(LoopEnd::End);
        self.line("end");
    }

    fn generic_for(&mut self) {
        let key = self.fresh_name();
        let value = self.fresh_name();
        if self.version.has_const_for_variables() {
            self.readonly_names.push(key.clone());
            self.readonly_names.push(value.clone());
        }
        let subject = self.known_name();
        let iterator = match self.rng.below(3) {
            0 => format!("ipairs({subject})"),
            1 => format!("pairs({subject})"),
            _ => format!("next, {subject}, nil"),
        };
        self.line(&format!("for {key}, {value} in {iterator} do"));
        self.loop_body(LoopEnd::End);
        self.line("end");
    }

    fn while_stmt(&mut self) {
        let condition = self.condition();
        self.line(&format!("while {condition} do"));
        self.loop_body(LoopEnd::End);
        self.line("end");
    }

    fn repeat_stmt(&mut self) {
        // No goto-continue idiom here: a repeat block's trailing label still
        // precedes `until`, which can see the block's locals, so Lua rejects
        // a goto that jumps over any of them.
        self.line("repeat");
        self.loop_body(LoopEnd::Until);
        let condition = self.condition();
        self.line(&format!("until {condition}"));
    }

    fn do_block(&mut self) {
        self.line("do");
        self.block(|generator| generator.small_body(2));
        self.line("end");
    }

    /// Loop bodies exercise the exit statements: `break` (as the last
    /// statement of an `if` block, valid in every version including the
    /// 5.1/Luau last-statement restriction), Luau `continue`, and the
    /// 5.2+ goto-continue idiom with the label in end-of-block position.
    fn loop_body(&mut self, loop_end: LoopEnd) {
        let has_goto_continue =
            loop_end == LoopEnd::End && self.version.has_goto() && self.rng.chance(30);
        let continue_label = has_goto_continue.then(|| {
            self.next_label = self.next_label.saturating_add(1);
            format!("continue_{}", self.next_label)
        });
        self.block(|generator| {
            if let Some(label) = &continue_label {
                let condition = generator.condition();
                generator.line(&format!("if {condition} then goto {label} end"));
            }
            generator.small_body(2);
            if generator.rng.chance(25) {
                let condition = generator.condition();
                generator.line(&format!("if {condition} then break end"));
            }
            match &continue_label {
                Some(label) => generator.line(&format!("::{label}::")),
                // Luau: last statement of the block only.
                None if generator.version.has_continue() && generator.rng.chance(20) => {
                    generator.line("continue");
                }
                None => {}
            }
        });
    }

    fn function_stmt(&mut self) {
        match self.rng.below(4) {
            1 => self.dotted_function(),
            2 if self.version.has_global() => {
                let name = self.fresh_name();
                self.line(&format!("global function {name}()"));
                self.function_body(Varargs::Forbidden);
                self.callables.push(name);
            }
            _ => self.local_function(),
        }
    }

    fn local_function(&mut self) {
        let name = self.fresh_name();
        let param = self.fresh_name();
        let varargs = if self.rng.chance(40) { Varargs::Allowed } else { Varargs::Forbidden };
        let generics = if self.version.is_luau() && self.rng.chance(25) { "<T>" } else { "" };
        let vararg_param = match varargs {
            Varargs::Allowed if self.version.has_named_varargs() && self.rng.chance(50) => {
                ", ...rest"
            }
            Varargs::Allowed => ", ...",
            Varargs::Forbidden => "",
        };
        let signature = if self.version.is_luau() {
            let annotation = self.type_expr(1);
            format!("local function {name}{generics}({param}: {annotation}{vararg_param}): number")
        } else {
            format!("local function {name}({param}{vararg_param})")
        };
        self.line(&signature);
        self.function_body(varargs);
        self.callables.push(name);
    }

    fn dotted_function(&mut self) {
        let base = self.known_name();
        let leaf = self.fresh_name();
        let is_method = self.rng.chance(50);
        let separator = if is_method { ":" } else { "." };
        let param = self.fresh_name();
        self.line(&format!("function {base}{separator}{leaf}({param})"));
        self.function_body(Varargs::Forbidden);
        if !is_method {
            self.callables.push(format!("{base}.{leaf}"));
        }
    }

    pub(super) fn function_body(&mut self, varargs: Varargs) {
        self.block(|indented| {
            indented.with_varargs(varargs, |generator| {
                generator.small_body(2);
                let result = generator.expr(2);
                if generator.rng.chance(30) {
                    let second = generator.expr(1);
                    generator.line(&format!("return {result}, {second}"));
                } else if generator.rng.chance(15) {
                    generator.line(&format!("return {result};"));
                } else {
                    generator.line(&format!("return {result}"));
                }
            });
        });
        self.line("end");
    }

    fn block<F>(&mut self, fill: F)
    where
        F: FnOnce(&mut Self),
    {
        self.indent.push('\t');
        self.stmt_depth = self.stmt_depth.saturating_add(1);
        fill(self);
        self.stmt_depth = self.stmt_depth.saturating_sub(1);
        self.indent.pop();
    }

    pub(super) fn small_body(&mut self, budget: usize) {
        let statement_count = self.rng.one_through(budget);
        for _ in 0..statement_count {
            self.stmt();
        }
    }
}
