use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Outside Roblox, `shared` is an ordinary name.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GlobalUsage;

declare_lint_rule!(
    GlobalUsage,
    Style,
    Warning,
    "use of the global environment tables `_G` or `shared`"
);

impl Rule for GlobalUsage {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let tree = context.scoping();
        let is_roblox = context.semantic.variant().is_roblox();
        let environments = [tree.name_id("_G"), tree.name_id("shared").filter(|_| is_roblox)];
        for reference in tree.global_references() {
            if environments.contains(&Some(reference.name)) {
                let name = tree.name(reference.name);
                context.diagnostic(
                    Finding::new(format!("global environment `{name}` used"), reference.span)
                        .with_help("prefer a module or an explicitly passed dependency"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] =
        &["local function init() shared = {} end", "print(pairs(items)); custom = 1"];

    const FAIL: &[&str] = &["_G.value = 1"];

    const ROBLOX_PASS: &[&str] =
        &["local _G, shared = {}, {}; _G.x = shared.x; object._G = object.shared"];

    const ROBLOX_FAIL: &[&str] = &["_G.value = shared.value; shared = {}"];

    #[test]
    fn test() {
        Tester::new(RuleId::GlobalUsage, PASS, FAIL)
            .with_roblox(ROBLOX_PASS, ROBLOX_FAIL)
            .test_and_snapshot();
    }
}
