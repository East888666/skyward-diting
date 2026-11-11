//! Serialize service creation with stopping it before partition writes.

use std::io;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
struct State {
    stopped: bool,
    pid: Option<u32>,
}

#[derive(Default)]
pub struct Control {
    state: Mutex<State>,
    exited: Condvar,
}

impl Control {
    /// Hold the same lock that stop uses across finding or spawning a child.
    pub fn launch<T>(&self, spawn: impl FnOnce() -> io::Result<(u32, T)>) -> io::Result<Option<T>> {
        let mut state = self.state.lock().expect("service control poisoned");
        if state.stopped {
            return Ok(None);
        }
        let (pid, child) = spawn()?;
        state.pid = Some(pid);
        Ok(Some(child))
    }

    pub fn finish(&self) {
        self.state.lock().expect("service control poisoned").pid = None;
        self.exited.notify_all();
    }

    /// Once stopped, this supervisor may never spawn from the old partitions.
    pub fn stop(&self) {
        self.state.lock().expect("service control poisoned").stopped = true;
    }

    pub fn signal(&self, send: impl FnOnce(u32) -> io::Result<()>) -> io::Result<()> {
        let state = self.state.lock().expect("service control poisoned");
        if let Some(pid) = state.pid {
            send(pid)?;
        }
        Ok(())
    }

    pub fn wait(&self, timeout: Duration) -> bool {
        let state = self.state.lock().expect("service control poisoned");
        self.exited
            .wait_timeout_while(state, timeout, |s| s.pid.is_some())
            .expect("service control poisoned")
            .0
            .pid
            .is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};
    use std::thread;

    #[test]
    fn stop_blocks_respawn_and_waits_for_exit() {
        let c = Control::default();
        assert_eq!(c.launch(|| Ok((42, 7))).unwrap(), Some(7));
        c.stop();
        let mut signalled = None;
        c.signal(|pid| {
            signalled = Some(pid);
            Ok(())
        })
        .unwrap();
        assert_eq!(signalled, Some(42));
        assert!(!c.wait(Duration::ZERO));
        c.finish();
        assert!(c.wait(Duration::ZERO));
        assert!(c
            .launch::<()>(|| panic!("must not respawn"))
            .unwrap()
            .is_none());
        c.signal(|_| panic!("must not signal an exited child"))
            .unwrap();
    }

    #[test]
    fn stop_cannot_miss_a_child_being_created() {
        let c = Arc::new(Control::default());
        let (started, start) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let worker = c.clone();
        let spawn = thread::spawn(move || {
            worker
                .launch(|| {
                    started.send(()).unwrap();
                    resume.recv().unwrap();
                    Ok((51, ()))
                })
                .unwrap()
        });
        start.recv().unwrap();
        let worker = c.clone();
        let stop = thread::spawn(move || worker.stop());
        release.send(()).unwrap();
        spawn.join().unwrap();
        stop.join().unwrap();
        let mut pid = None;
        c.signal(|p| {
            pid = Some(p);
            Ok(())
        })
        .unwrap();
        assert_eq!(pid, Some(51));
        assert!(c.launch::<()>(|| panic!("stopped")).unwrap().is_none());
    }

    #[test]
    fn spawn_error_does_not_leave_a_phantom_child() {
        let c = Control::default();
        assert!(c
            .launch::<()>(|| Err(io::Error::other("spawn failed")))
            .is_err());
        c.stop();
        assert!(c.wait(Duration::ZERO));
    }
}
