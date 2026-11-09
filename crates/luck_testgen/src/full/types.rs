use super::FullGenerator;

const BUILTIN_TYPES: [&str; 8] =
    ["number", "integer", "string", "boolean", "any", "unknown", "thread", "never"];

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the generator is one type with its grammar split by file"
)]
impl FullGenerator {
    pub(super) fn type_alias_stmt(&mut self) {
        let name = format!("Alias{}", self.fresh_id());
        let export_prefix =
            if self.indent.is_empty() && self.rng.chance(40) { "export " } else { "" };
        let is_generic = self.rng.chance(30);
        let body = self.type_expr(2);
        if is_generic {
            self.line(&format!("{export_prefix}type {name}<T> = {{ value: T, tail: {body} }}"));
            self.alias_types.push(format!("{name}<string>"));
        } else {
            self.line(&format!("{export_prefix}type {name} = {body}"));
            self.alias_types.push(name);
        }
    }

    pub(super) fn type_expr(&mut self, depth: usize) -> String {
        let Some(inner_depth) = depth.checked_sub(1) else {
            return self.leaf_type();
        };
        match self.rng.below(11) {
            0..=2 => self.leaf_type(),
            3 => {
                let inner = self.rng.pick(&BUILTIN_TYPES);
                format!("{inner}?")
            }
            4 => {
                let lhs = self.type_expr(inner_depth);
                let rhs = self.type_expr(inner_depth);
                format!("({lhs}) | ({rhs})")
            }
            5 => {
                let lhs = self.leaf_type();
                let rhs = self.leaf_type();
                format!("({lhs}) & ({rhs})")
            }
            6 => {
                let element = self.type_expr(inner_depth);
                format!("{{ {element} }}")
            }
            7 => {
                let field = self.type_expr(inner_depth);
                let indexed = self.leaf_type();
                format!("{{ label: {field}, [string]: {indexed} }}")
            }
            8 => {
                let param = self.leaf_type();
                let return_type = self.type_expr(inner_depth);
                match self.rng.below(3) {
                    0 => format!("({param}) -> {return_type}"),
                    1 => format!("({param}, ...{param}) -> ({return_type})"),
                    _ => "() -> ()".to_owned(),
                }
            }
            _ => {
                let subject = self.known_name();
                format!("typeof({subject})")
            }
        }
    }

    pub(super) fn leaf_type(&mut self) -> String {
        if !self.alias_types.is_empty() && self.rng.chance(25) {
            return self.rng.pick(&self.alias_types);
        }
        match self.rng.below(9) {
            0..=5 => self.rng.pick(&BUILTIN_TYPES).to_owned(),
            6 => "\"singleton\"".to_owned(),
            7 => "true".to_owned(),
            _ => "nil".to_owned(),
        }
    }
}
