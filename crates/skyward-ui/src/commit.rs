//! Run one blocking display commit while the UI draws into its next buffer.
//! No second commit is accepted until the first completion has been consumed.

use anyhow::{Context, Result, anyhow, ensure};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(crate) struct CommitWorker<T> {
    requests: Option<SyncSender<T>>,
    completions: Receiver<Result<Duration>>,
    thread: Option<JoinHandle<()>>,
    pending: bool,
    failed: bool,
}

impl<T: Send + 'static> CommitWorker<T> {
    pub(crate) fn start(mut commit: impl FnMut(T) -> Result<()> + Send + 'static) -> Result<Self> {
        let (requests, rx) = mpsc::sync_channel(1);
        let (tx, completions) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("display-commit".into())
            .spawn(move || {
                while let Ok(request) = rx.recv() {
                    let started = Instant::now();
                    let result = commit(request).map(|()| started.elapsed());
                    let failed = result.is_err();
                    if tx.send(result).is_err() || failed {
                        break;
                    }
                }
            })
            .context("start display commit worker")?;
        Ok(Self {
            requests: Some(requests),
            completions,
            thread: Some(thread),
            pending: false,
            failed: false,
        })
    }

    pub(crate) fn submit(&mut self, request: T) -> Result<()> {
        ensure!(!self.failed, "display commit worker previously failed");
        ensure!(
            !self.pending,
            "previous display commit has not been settled"
        );
        if self.requests.as_ref().unwrap().send(request).is_err() {
            self.failed = true;
            return Err(anyhow!("display commit worker stopped"));
        }
        self.pending = true;
        Ok(())
    }

    pub(crate) fn settle(&mut self) -> Result<Option<Duration>> {
        ensure!(!self.failed, "display commit worker previously failed");
        if !self.pending {
            return Ok(None);
        }
        let result = self
            .completions
            .recv()
            .context("display commit worker stopped")
            .and_then(|r| r);
        self.pending = false;
        self.failed = result.is_err();
        result.map(Some)
    }
}

impl<T> Drop for CommitWorker<T> {
    fn drop(&mut self) {
        // Drain the one outstanding ioctl before its owner unmaps buffers
        // or closes the shared DRM file. No pending work is abandoned.
        drop(self.requests.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(2);

    #[test]
    fn drawing_can_continue_but_a_second_commit_waits_for_completion() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut worker = CommitWorker::start(move |number| {
            started_tx.send(number)?;
            release_rx.recv_timeout(WAIT)?;
            Ok(())
        })
        .unwrap();
        worker.submit(1).unwrap();
        assert_eq!(started_rx.recv_timeout(WAIT).unwrap(), 1);
        assert!(worker.submit(2).is_err());
        release_tx.send(()).unwrap();
        assert!(worker.settle().unwrap().is_some());
        assert!(worker.settle().unwrap().is_none());
        worker.submit(2).unwrap();
        assert_eq!(started_rx.recv_timeout(WAIT).unwrap(), 2);
        release_tx.send(()).unwrap();
        worker.settle().unwrap();
    }

    #[test]
    fn driver_errors_are_reported_and_latched() {
        let mut worker = CommitWorker::start(|()| anyhow::bail!("ioctl failed")).unwrap();
        worker.submit(()).unwrap();
        assert!(
            worker
                .settle()
                .unwrap_err()
                .to_string()
                .contains("ioctl failed")
        );
        assert!(worker.submit(()).is_err());
        assert!(worker.settle().is_err());
    }

    #[test]
    fn dropping_waits_for_the_ioctl_before_releasing_its_owner() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let mut worker = CommitWorker::start(move |()| {
            started_tx.send(())?;
            release_rx.recv_timeout(WAIT)?;
            Ok(())
        })
        .unwrap();
        worker.submit(()).unwrap();
        started_rx.recv_timeout(WAIT).unwrap();
        let dropper = thread::spawn(move || {
            drop(worker);
            dropped_tx.send(()).unwrap();
        });
        assert!(matches!(
            dropped_rx.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release_tx.send(()).unwrap();
        dropped_rx.recv_timeout(WAIT).unwrap();
        dropper.join().unwrap();
    }

    #[test]
    fn a_panicked_worker_returns_an_error_without_waiting_forever() {
        let mut worker = CommitWorker::start(|()| panic!("driver wrapper panicked")).unwrap();
        worker.submit(()).unwrap();
        assert!(worker.settle().is_err());
        assert!(worker.submit(()).is_err());
    }
}
