use std::borrow::Cow;
use std::cell::RefCell;
use std::num::NonZero;
use std::path::Path;

use luck_diagnostics::Diagnostic;
use luck_type_checker::{CancellationToken, CheckError, Checker, Config, DirectoryConfigs, Host};
use rustc_hash::FxHashMap;

/// Modules held in memory by name, where `require("./name")` names the module
/// `name`, all in one directory, whose `.luaurc` or `.config.luau` governs
/// them all.
#[derive(Default)]
pub(crate) struct MemoryHost {
    pub(crate) modules: FxHashMap<String, String>,
    pub(crate) luaurc: Option<String>,
    pub(crate) config_luau: Option<String>,
    /// The message of each E014 the directory's config got.
    pub(crate) config_errors: RefCell<Vec<String>>,
    pub(crate) configs: DirectoryConfigs,
}

impl Host for MemoryHost {
    fn source(&self, module: &str) -> Option<Cow<'_, [u8]>> {
        self.modules.get(module).map(|source| Cow::Borrowed(source.as_bytes()))
    }

    fn resolve(&self, _: &str, path: &str) -> Option<String> {
        let module = path.strip_prefix("./")?;
        self.modules.contains_key(module).then(|| module.to_owned())
    }

    fn config(&self, module: &str, config: &mut Config<'_>) {
        let read = |path: &Path| match path.to_str()? {
            ".luaurc" => self.luaurc.clone(),
            ".config.luau" => self.config_luau.clone(),
            _ => None,
        };
        let errors = self.configs.apply(Path::new(module), config, read);
        self.config_errors
            .borrow_mut()
            .extend(errors.into_iter().map(|(_, diag)| diag.message.to_string()));
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
