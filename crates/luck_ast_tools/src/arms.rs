use proc_macro2::TokenStream;
use quote::quote;

/// A `match`'s arms, where arms that bind values of the same types to the
/// same body join into one or-pattern, as clippy's `match_same_arms` asks.
#[derive(Default)]
pub(crate) struct Arms {
    arms: Vec<Arm>,
}

struct Arm {
    /// The types of the values the patterns bind, and the body.
    key: String,
    patterns: Vec<TokenStream>,
    body: TokenStream,
}

impl Arms {
    /// `bindings` names the types of the values `pattern` binds, in order.
    pub(crate) fn push(&mut self, bindings: &str, pattern: TokenStream, body: TokenStream) {
        let key = format!("{bindings} => {body}");
        if let Some(arm) = self.arms.iter_mut().find(|arm| arm.key == key) {
            arm.patterns.push(pattern);
        } else {
            self.arms.push(Arm { key, patterns: vec![pattern], body });
        }
    }

    pub(crate) fn into_tokens(self) -> TokenStream {
        let arms = self
            .arms
            .into_iter()
            .map(|Arm { patterns, body, .. }| quote!(#(#patterns)|* => #body,));
        quote!(#(#arms)*)
    }
}
