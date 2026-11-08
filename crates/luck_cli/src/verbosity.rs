/// `Quiet` under `--quiet`, which hides progress, summaries, and bundler
/// warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verbosity {
    Quiet,
    Normal,
}
