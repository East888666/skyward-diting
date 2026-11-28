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
                    let result = unlock_data(&closed, user, &credential);
                    let mut credential = credential;
                    credential.fill(0);
                    Ended::Unlocked(result)
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

/// Dispatch credential unlock through the same configured HAL as opening.
/// Modern devices retain their existing KeyMint and profile-unlock path.
pub fn unlock_data(closed: &[Closed], user: u32, credential: &[u8]) -> Result<Vec<Closed>, String> {
    let config = skyward_config::DeviceConfig::load("/skyward/device.toml").map_err(|e| format!("{e:#}"))?;
    if config.recovery.uses_legacy_fbe() {
        return data::unlock_legacy(closed, user, credential).map_err(|e| format!("{e:#}"));
    }
    with_key_service(|service| data::unlock(service, closed, user, credential))
}

/// Plain legacy data needs no AIDL service. Preserve both failure reasons
/// when the configured encryption requires hardware this recovery lacks.
pub fn open_data() -> Result<Opening, String> {
    let config = skyward_config::DeviceConfig::load("/skyward/device.toml");
    let operation = if config.as_ref().is_ok_and(|cfg| cfg.recovery.legacy_fde) {
        Some(skyward_crypto::fde::lock().map_err(|e| format!("{e:#}"))?)
    } else { None };
    if let Some(operation) = &operation && skyward_crypto::fde::active() {
        return data::open_legacy_fde(operation).and_then(|opening| opening.ok_or_else(|| anyhow::anyhow!("active cryptfs has no footer")))
            .map_err(|e| format!("{e:#}"));
    }
    match data::open_without_keys() {
        Ok(opening) => Ok(opening),
        Err(plain) => {
            if let Some(operation) = &operation {
                match data::open_legacy_fde(operation) {
                    Ok(Some(opening)) => return Ok(opening),
                    Ok(None) => {},
                    Err(error) => return Err(format!("legacy FDE: {error:#}")),
                }
            }
            // A damaged/unreplayed filesystem is not evidence that a key
            // service should be tried. Its later missing-key error would
            // obscure the directory I/O failure that actually needs fixing.
            if plain.chain().any(|cause| cause.downcast_ref::<std::io::Error>()
                .is_some_and(|io| matches!(io.raw_os_error(), Some(libc::EUCLEAN | libc::EIO)))) {
                return Err(format!("data filesystem could not be read; check its filesystem before retrying: {plain:#}"));
            }
            let config = skyward_config::DeviceConfig::load("/skyward/device.toml");
            if let Ok(cfg) = config
                && cfg.recovery.uses_legacy_fbe()
            {
                let opening = if cfg.recovery.legacy_fbe_auto {
                    data::open_legacy_with_mount_selection(true)
                } else {
                    data::open_legacy_without_credential()
                };
                return opening.map_err(|error| format!("plain data: {plain:#}; legacy FBE: {error:#}"));
            }
            with_data_keys(data::open_without_credential)
                .map_err(|hardware| format!("plain data: {plain:#}; encrypted data: {hardware}"))
        }
    }
}

/// Do `work` with what reaching the data partition takes: the phone's key
/// service, reached afresh for it, where the phone's own table has the
/// partition encrypted, and nothing where it has not, so that a partition
/// kept as the plain filesystem it is opens on a phone whose key service
/// does not start. Says why it failed in words for the page.
pub fn with_data_keys<T>(work: impl FnOnce(Option<&KeyMint>) -> Result<T>) -> Result<T, String> {
    let done = (|| {
        if !data::keyed()? {
            return work(None);
        }
        let binder = Binder::open()?;
        let service = KeyMint::open(&binder)?;
        work(Some(&service))
    })();
    done.map_err(|e| format!("{e:#}"))
}
