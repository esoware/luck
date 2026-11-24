use std::io::{self, BufWriter, StderrLock, StdoutLock};
use std::num::NonZero;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc;
use std::{iter, thread};

use luck_allocator::{Allocator, AllocatorPool};
use luck_ast::MIN_STACK_BYTES;
use rayon::ThreadPoolBuilder;

use crate::exit::{Exit, operation_error};
use crate::project::ProjectFile;

/// What a run buffers before writing to stdout or stderr. One file's rendered
/// diagnostics often outgrow the default buffer, which would write each
/// through on its own.
const OUTPUT_BUFFER_BYTES: usize = 1 << 18;

/// Builds rayon's global pool, which a command that works on many files runs
/// them on, with `threads` threads, each with the stack a deep file needs.
pub(crate) fn start_thread_pool(threads: NonZero<usize>) -> Result<(), Exit> {
    ThreadPoolBuilder::new()
        .num_threads(threads.get())
        .stack_size(MIN_STACK_BYTES)
        .build_global()
        .map_err(|e| operation_error(format!("can't start {threads} threads: {e}")))
}

/// Stdout and stderr, each locked once and buffered for a whole run of
/// [`for_each_outcome_in_order`].
pub(crate) struct LockedOutput {
    pub stdout: BufWriter<StdoutLock<'static>>,
    pub stderr: BufWriter<StderrLock<'static>>,
}

/// Runs `work` over `files` on the rayon pool and hands `take` each outcome
/// on the calling thread, in input order, with the run's [`LockedOutput`].
///
/// The calling thread holds the stdio locks while the pool runs `work`, so
/// `work` must never print, or it waits on them forever. An error from `take`
/// ends the run once the files already started finish. Each file gets an
/// arena from a pool, reset when its outcome is done, so a run allocates about
/// one arena per thread. A file starts as soon as a thread is free and is less
/// than a window of files past the outcome `take` waits for, which keeps
/// memory flat on huge projects without a thread ever waiting for a batch to
/// finish.
pub(crate) fn for_each_outcome_in_order<Outcome>(
    files: &[ProjectFile],
    work: impl Fn(&ProjectFile, &Allocator) -> Outcome + Sync,
    mut take: impl FnMut(Outcome, &mut LockedOutput) -> Result<(), Exit>,
) -> Result<(), Exit>
where
    Outcome: Send,
{
    let mut output = LockedOutput {
        stdout: BufWriter::with_capacity(OUTPUT_BUFFER_BYTES, io::stdout().lock()),
        stderr: BufWriter::with_capacity(OUTPUT_BUFFER_BYTES, io::stderr().lock()),
    };
    let thread_count = rayon::current_num_threads();
    let pool = AllocatorPool::new(thread_count);
    // A smaller window leaves threads idle behind a slow file, and a larger
    // one gains nothing.
    let window = thread_count * 16;
    let (sender, receiver) = mpsc::channel::<(usize, thread::Result<Outcome>)>();
    let mut ready: Vec<Option<Outcome>> = iter::repeat_with(|| None).take(window).collect();
    let mut next_outcome = |index: usize| loop {
        if let Some(outcome) = ready[index % window].take() {
            return outcome;
        }
        let (finished, result) =
            receiver.recv().expect("`sender` outlives this, so a job still out always arrives");
        let outcome = result.unwrap_or_else(|payload| panic::resume_unwind(payload));
        ready[finished % window] = Some(outcome);
    };
    // Spawned from outside the pool, the jobs queue in file order.
    rayon::in_place_scope(|scope| {
        let start = |index: usize| {
            if let Some(file) = files.get(index) {
                let (job_work, job_pool, job_sender) = (&work, &pool, sender.clone());
                scope.spawn(move |_| {
                    // A panic would otherwise leave the caller waiting forever.
                    let result =
                        panic::catch_unwind(AssertUnwindSafe(|| job_work(file, &job_pool.get())));
                    let _ = job_sender.send((index, result));
                });
            }
        };
        (0..window).for_each(start);
        for index in 0..files.len() {
            let outcome = next_outcome(index);
            start(index + window);
            take(outcome, &mut output)?;
        }
        Ok(())
    })
}
