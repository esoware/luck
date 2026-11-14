//! A token formatted on its own is a name. Singletons, access modifiers, and
//! interpolated string segments, the other tokens a tree holds, print where
//! they sit, through their own spelling.

use luck_ast::name_spelling;
use luck_token::Token;

use crate::ir::{Format, Formatter};

impl Format for Token {
    fn fmt(&self, f: &mut Formatter) {
        if let Some(name) = f.spelled(name_spelling(self)) {
            f.write_text(name);
        }
    }
}
