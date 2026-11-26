use std::borrow::Cow;
use std::cell::RefCell;
use std::num::NonZero;

use luck_diagnostics::Diagnostic;
use luck_type_checker::{CancellationToken, CheckError, Checker, Config, Host, SavedConfig};
use rustc_hash::FxHashMap;

/// Modules held in memory by name, where `require("./name")` names the module
/// `name`, and one `.luaurc`, or else one `.config.luau`, governs them all.
#[derive(Default)]
pub(crate) struct MemoryHost {
    pub(crate) modules: FxHashMap<String, String>,
    pub(crate) luaurc: Option<String>,
    pub(crate) config_luau: Option<String>,
    /// What Luau said each time it couldn't read the config.
    pub(crate) config_errors: RefCell<Vec<String>>,
    /// The config once read, which every module after takes, as a host keeps
    /// a directory's.
    pub(crate) saved: RefCell<Option<SavedConfig>>,
}

impl Host for MemoryHost {
    fn source(&self, module: &str) -> Option<Cow<'_, [u8]>> {
        self.modules.get(module).map(|source| Cow::Borrowed(source.as_bytes()))
    }

    fn resolve(&self, _: &str, path: &str) -> Option<String> {
        let module = path.strip_prefix("./")?;
        self.modules.contains_key(module).then(|| module.to_owned())
    }

    fn config(&self, _: &str, config: &mut Config<'_>) {
        if let Some(saved) = &*self.saved.borrow() {
            config.set(saved);
            return;
        }
        let applied = match (&self.luaurc, &self.config_luau) {
            (Some(luaurc), _) => config.apply_luaurc(luaurc),
            (None, Some(source)) => config.apply_luau_config(source),
            (None, None) => Ok(()),
        };
        if let Err(error) = applied {
            self.config_errors.borrow_mut().push(error);
        }
        *self.saved.borrow_mut() = config.saved();
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
