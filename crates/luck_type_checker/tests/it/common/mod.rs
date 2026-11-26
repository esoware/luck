use std::cell::RefCell;
use std::num::NonZero;

use luck_diagnostics::Diagnostic;
use luck_type_checker::{CancellationToken, CheckError, Checker, Config, Host};
use rustc_hash::FxHashMap;

/// Modules held in memory by name, where `require("./name")` names the module
/// `name`, and one `.luaurc` governs them all.
#[derive(Default)]
pub(crate) struct MemoryHost {
    pub(crate) modules: FxHashMap<String, String>,
    pub(crate) luaurc: Option<String>,
    /// What Luau said each time it couldn't read the `.luaurc`.
    pub(crate) luaurc_errors: RefCell<Vec<String>>,
}

impl Host for MemoryHost {
    fn source(&self, module: &str) -> Option<Vec<u8>> {
        self.modules.get(module).map(|source| source.clone().into_bytes())
    }

    fn resolve(&self, _: &str, path: &str) -> Option<String> {
        let module = path.strip_prefix("./")?;
        self.modules.contains_key(module).then(|| module.to_owned())
    }

    fn config(&self, _: &str, config: &mut Config<'_>) {
        if let Some(luaurc) = &self.luaurc
            && let Err(error) = config.apply_luaurc(luaurc)
        {
            self.luaurc_errors.borrow_mut().push(error);
        }
    }
}

pub(crate) fn checker(modules: &[(&str, &str)]) -> Checker<MemoryHost> {
    Checker::new(MemoryHost {
        modules: modules
            .iter()
            .map(|&(name, source)| (name.to_owned(), source.to_owned()))
            .collect(),
        ..MemoryHost::default()
    })
}

pub(crate) fn check(
    checker: &mut Checker<MemoryHost>,
    modules: &[&str],
) -> Result<Vec<Vec<Diagnostic>>, CheckError> {
    checker.check(modules, NonZero::new(2).unwrap(), &CancellationToken::new())
}

/// Each diagnostic's code, message, and the text its span covers in `source`.
pub(crate) fn described<'source>(
    diags: &[Diagnostic],
    source: &'source str,
) -> Vec<(&'static str, String, &'source str)> {
    diags
        .iter()
        .map(|diag| {
            let text = diag.span.map_or("", |span| &source[span.start as usize..span.end as usize]);
            (diag.code, diag.message.to_string(), text)
        })
        .collect()
}
