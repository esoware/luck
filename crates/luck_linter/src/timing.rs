use std::cmp::Reverse;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::rules::{RuleId, RuleTable};

/// How long each rule ran over every file a run lints with it, and how many
/// times the linter called it, as oxlint's `RuleTimingStore` keeps.
///
/// Every thread of a run adds to one, once per file. A rule that first asks
/// for something the rules share, such as a function's control-flow graph,
/// pays for building it.
#[derive(Debug, Default)]
pub struct RuleTimings {
    rules: RuleTable<RuleTimer>,
}

#[derive(Debug, Default)]
struct RuleTimer {
    nanos: AtomicU64,
    calls: AtomicU64,
}

/// One rule's share of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleTiming {
    pub rule: RuleId,
    pub duration: Duration,
    pub calls: u64,
}

impl RuleTimings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Each rule the linter called, slowest first, and rules that took as
    /// long by name.
    pub fn sorted(&self) -> Vec<RuleTiming> {
        let mut timings: Vec<RuleTiming> = RuleId::ALL
            .into_iter()
            .map(|rule| {
                let timer = &self.rules[rule];
                let nanos = timer.nanos.load(Ordering::Relaxed);
                let calls = timer.calls.load(Ordering::Relaxed);
                RuleTiming { rule, duration: Duration::from_nanos(nanos), calls }
            })
            .filter(|timing| timing.calls > 0)
            .collect();
        timings.sort_by_key(|timing| (Reverse(timing.duration), timing.rule.name()));
        timings
    }

    pub(crate) fn add(&self, rule: RuleId, spent: Spent) {
        let timer = &self.rules[rule];
        let nanos = u64::try_from(spent.duration.as_nanos()).unwrap_or(u64::MAX);
        timer.nanos.fetch_add(nanos, Ordering::Relaxed);
        timer.calls.fetch_add(spent.calls, Ordering::Relaxed);
    }
}

/// What one rule spent on one file, which the linter adds to the run's
/// [`RuleTimings`] once the file is done.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Spent {
    duration: Duration,
    calls: u64,
}

impl Spent {
    /// What `hook` returns, with the time it took and one more call added.
    pub(crate) fn time<Hook, Returned>(&mut self, hook: Hook) -> Returned
    where
        Hook: FnOnce() -> Returned,
    {
        let started = Instant::now();
        let returned = hook();
        self.duration += started.elapsed();
        self.calls += 1;
        returned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_takes_the_slowest_first_and_leaves_out_rules_never_called() {
        let timings = RuleTimings::new();
        let spent = |nanos: u64, calls: u64| Spent { duration: Duration::from_nanos(nanos), calls };
        timings.add(RuleId::Shadowing, spent(5, 2));
        timings.add(RuleId::UndefinedVariable, spent(9, 1));
        timings.add(RuleId::Shadowing, spent(5, 1));
        let sorted: Vec<(RuleId, u128, u64)> = timings
            .sorted()
            .into_iter()
            .map(|timing| (timing.rule, timing.duration.as_nanos(), timing.calls))
            .collect();
        assert_eq!(sorted, [(RuleId::Shadowing, 10, 3), (RuleId::UndefinedVariable, 9, 1)]);
    }
}
