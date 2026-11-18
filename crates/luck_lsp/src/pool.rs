use core::iter;
use std::io;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, select_biased, unbounded};
use luck_ast::MIN_STACK_BYTES;

type Job = Box<dyn FnOnce() + Send>;

/// Which queue a job waits in. A worker takes a request before any
/// background job, so a lint of every open document doesn't keep a hover
/// waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    /// An answer the client is waiting for.
    Request,
    /// Diagnostics, which the server pushes when they're ready.
    Background,
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
    workers: Vec<JoinHandle<()>>,
}

impl Pool {
    /// # Errors
    ///
    /// Returns the error from starting a thread.
    pub(crate) fn new(threads: usize) -> io::Result<Self> {
        let (requests, request_jobs) = unbounded::<Job>();
        let (background, background_jobs) = unbounded::<Job>();
        let workers = iter::repeat_with(|| {
            let (takes_requests, takes_background) =
                (request_jobs.clone(), background_jobs.clone());
            thread::Builder::new()
                .name("luck-lsp-worker".to_owned())
                .stack_size(MIN_STACK_BYTES)
                .spawn(move || work(&takes_requests, &takes_background))
        })
        .take(threads.max(1))
        .collect::<io::Result<_>>()?;
        Ok(Self { requests, background, workers })
    }

    pub(crate) fn spawn<F>(&self, priority: Priority, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let queue = match priority {
            Priority::Request => &self.requests,
            Priority::Background => &self.background,
        };
        // A worker holds each receiver until both queues close, which only
        // `join` does.
        let _queued = queue.send(Box::new(job));
    }

    /// Closes both queues, and waits for the workers to run what's queued
    /// and stop.
    pub(crate) fn join(self) {
        let Self { requests, background, workers } = self;
        drop((requests, background));
        for worker in workers {
            // A worker that panicked has nothing left to stop.
            let _stopped = worker.join();
        }
    }
}

/// Runs jobs until both queues are closed and empty.
fn work(requests: &Receiver<Job>, background: &Receiver<Job>) {
    loop {
        let next = select_biased! {
            recv(requests) -> request => request.or_else(|_| background.recv()),
            recv(background) -> queued => queued.or_else(|_| requests.recv()),
        };
        match next {
            Ok(job) => job(),
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use crossbeam_channel::bounded;

    use super::*;

    #[test]
    fn a_request_goes_before_queued_background_jobs() {
        let pool = Pool::new(1).expect("a worker starts");
        let (started, is_started) = bounded(0);
        let (release, is_released) = bounded::<()>(0);
        pool.spawn(Priority::Background, move || {
            started.send(()).expect("the test waits for it");
            is_released.recv().expect("the test releases it");
        });
        is_started.recv_timeout(Duration::from_secs(30)).expect("the worker is busy");
        let (order, ran) = unbounded();
        let background_order = order.clone();
        pool.spawn(Priority::Background, move || {
            background_order.send(Priority::Background).expect("the test reads it");
        });
        pool.spawn(Priority::Request, move || order.send(Priority::Request).expect("read"));
        release.send(()).expect("the first job waits");
        pool.join();
        assert_eq!(ran.try_iter().collect::<Vec<_>>(), [Priority::Request, Priority::Background]);
    }
}
