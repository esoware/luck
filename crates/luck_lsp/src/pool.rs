use std::io;
use std::iter;
use std::num::NonZero;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, RecvError, Sender, select_biased, unbounded};
use luck_ast::MIN_STACK_BYTES;

type Job = Box<dyn FnOnce() + Send>;

/// Which queue a job waits in.
///
/// A worker takes a request before any background job, and either before an
/// idle one, so a lint of every open document doesn't keep a hover waiting,
/// and reading the workspace keeps neither waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    /// An answer the client is waiting for.
    Request,
    /// Diagnostics, which the server pushes when they're ready.
    Background,
    /// Reading the workspace's files ahead of the first request that needs
    /// them.
    Idle,
}

/// The threads every job runs on, each with the stack any stage needs.
///
/// Its own threads, not rayon's: a job can wait for a version's analysis or
/// lints while another job builds them, and a rayon thread waiting inside
/// the lint it's building could steal that waiting job, which would then
/// wait on its own thread forever.
#[derive(Debug)]
pub(crate) struct Pool {
    requests: Sender<Job>,
    background: Sender<Job>,
    idle: Sender<Job>,
    workers: Vec<JoinHandle<()>>,
}

/// What a worker takes jobs from, in the order it prefers them.
struct Queues {
    requests: Receiver<Job>,
    background: Receiver<Job>,
    idle: Receiver<Job>,
}

impl Pool {
    pub(crate) fn new(threads: NonZero<usize>) -> io::Result<Self> {
        let (requests, request_jobs) = unbounded::<Job>();
        let (background, background_jobs) = unbounded::<Job>();
        let (idle, idle_jobs) = unbounded::<Job>();
        let workers = iter::repeat_with(|| {
            let queues = Queues {
                requests: request_jobs.clone(),
                background: background_jobs.clone(),
                idle: idle_jobs.clone(),
            };
            thread::Builder::new()
                .name("luck-lsp-worker".to_owned())
                .stack_size(MIN_STACK_BYTES)
                .spawn(move || queues.work())
        })
        .take(threads.get())
        .collect::<io::Result<_>>()?;
        Ok(Self { requests, background, idle, workers })
    }

    pub(crate) fn spawn<F>(&self, priority: Priority, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let queue = match priority {
            Priority::Request => &self.requests,
            Priority::Background => &self.background,
            Priority::Idle => &self.idle,
        };
        // A worker holds each receiver until every queue closes, which only
        // `join` does.
        let _ = queue.send(Box::new(job));
    }

    /// Closes every queue, and waits for the workers to run what's queued
    /// and stop.
    pub(crate) fn join(self) {
        let Self { requests, background, idle, workers } = self;
        drop((requests, background, idle));
        for worker in workers {
            // A worker that panicked has nothing left to stop.
            let _ = worker.join();
        }
    }
}

impl Queues {
    /// Runs jobs until every queue is closed and empty.
    fn work(&self) {
        while let Ok(job) = self.next() {
            job();
        }
    }

    /// The first job of the queue it prefers that holds one, waiting for
    /// one when none does.
    fn next(&self) -> Result<Job, RecvError> {
        let Self { requests, background, idle } = self;
        let drained = || requests.recv().or_else(|_| background.recv()).or_else(|_| idle.recv());
        select_biased! {
            recv(requests) -> request => request.or_else(|_| drained()),
            recv(background) -> queued => queued.or_else(|_| drained()),
            recv(idle) -> waiting => waiting.or_else(|_| drained()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crossbeam_channel::bounded;

    use super::*;

    #[test]
    fn a_request_goes_before_queued_background_jobs_and_both_before_idle_ones() {
        let pool = Pool::new(NonZero::<usize>::MIN).expect("a worker starts");
        let (started, is_started) = bounded(0);
        let (release, is_released) = bounded::<()>(0);
        pool.spawn(Priority::Background, move || {
            started.send(()).expect("the test waits for it");
            is_released.recv().expect("the test releases it");
        });
        is_started.recv_timeout(Duration::from_secs(30)).expect("the worker is busy");
        let (order, ran) = unbounded();
        for priority in [Priority::Idle, Priority::Background, Priority::Request] {
            let sender = order.clone();
            pool.spawn(priority, move || sender.send(priority).expect("the test reads it"));
        }
        release.send(()).expect("the first job waits");
        pool.join();
        assert_eq!(
            ran.try_iter().collect::<Vec<_>>(),
            [Priority::Request, Priority::Background, Priority::Idle]
        );
    }
}
