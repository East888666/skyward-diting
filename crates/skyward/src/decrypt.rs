//! Opening the data partition and its users' own files on a thread of its
//! own, so the screen keeps up: the keys come from the secure world, and
//! what a user enters at their screen lock is checked slowly on purpose.

use anyhow::Result;
use skyward_binder::driver::Binder;
use skyward_crypto::data::{self, Closed, Opening};
use skyward_crypto::keymint::KeyMint;
use skyward_sys::worker::Worker;
use std::os::fd::RawFd;

pub enum Job {
    /// Open the partition as far as it opens with nothing a user knows.
    Open,
    /// Open the own files of `user` with what they entered at their screen
    /// lock; `closed` are the users whose own files are still closed.
    Unlock { closed: Vec<Closed>, user: u32, credential: Vec<u8> },
}

/// How a job ended: what it opened, or why it did not.
pub enum Ended {
    Opened(Result<Opening, String>),
    Unlocked(Result<Vec<Closed>, String>),
}

/// A job done on a thread of its own, which the screen lets go once the
/// job has ended.
pub struct Decryptor {
    /// Tells how the job ended, once.
    worker: Worker<Ended>,
}

impl Decryptor {
    pub fn start(job: Job) -> Result<Decryptor> {
        let worker = Worker::start("decrypt", move |tell| {
            tell.tell(match job {
                Job::Open => Ended::Opened(open_data()),
                Job::Unlock { closed, user, credential } => {
                    Ended::Unlocked(with_key_service(|service| data::unlock(service, &closed, user, &credential)))
                }
            });
        })?;
        Ok(Decryptor { worker })
    }

    /// Readable once the job has ended.
    pub fn fd(&self) -> RawFd {
        self.worker.fd()
    }

    /// How the job ended, once it has.
    pub fn ended(&self) -> Option<Ended> {
        self.worker.events().pop()
    }
}

/// Do `work` with the phone's key service, reached afresh for it, and say
/// why it failed in words for the page.
pub fn with_key_service<T>(work: impl FnOnce(&KeyMint) -> Result<T>) -> Result<T, String> {
    let done = (|| {
        let binder = Binder::open()?;
        let service = KeyMint::open(&binder)?;
        work(&service)
    })();
    done.map_err(|e| format!("{e:#}"))
}

/// Plain legacy data needs no AIDL service. Preserve both failure reasons
/// when the configured encryption requires hardware this recovery lacks.
pub fn open_data() -> Result<Opening, String> {
    match data::open_without_keys() {
        Ok(opening) => Ok(opening),
        Err(plain) => {
            // A damaged/unreplayed filesystem is not evidence that a key
            // service should be tried. Its later missing-key error would
            // obscure the directory I/O failure that actually needs fixing.
            if plain.chain().any(|cause| cause.downcast_ref::<std::io::Error>()
                .is_some_and(|io| matches!(io.raw_os_error(), Some(libc::EUCLEAN | libc::EIO)))) {
                return Err(format!("data filesystem could not be read; check its filesystem before retrying: {plain:#}"));
            }
            let legacy = skyward_config::DeviceConfig::load("/skyward/device.toml")
                .is_ok_and(|cfg| cfg.recovery.legacy_version.is_some());
            if legacy {
                return data::open_legacy_without_credential()
                    .map_err(|error| format!("plain data: {plain:#}; legacy FBE: {error:#}"));
            }
            with_key_service(data::open_without_credential)
                .map_err(|hardware| format!("plain data: {plain:#}; encrypted data: {hardware}"))
        }
    }
}
