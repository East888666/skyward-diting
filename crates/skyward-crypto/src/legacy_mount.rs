//! Read-only legacy mount selection. PRIVATE/ICE's on-disk mode and policy
//! do not say whether its file contents were written through old PFK or
//! software AES. Authenticate actual DE key storage before ordinary writes.

use anyhow::{Context, Result};

pub(crate) enum ProbeError {
    /// Parsing the encrypted key files failed before calling hardware.
    Contents(anyhow::Error),
    /// Hardware/service/key-install failures cannot select a mount option.
    Service(anyhow::Error),
}

impl ProbeError {
    pub(crate) fn contents(error: anyhow::Error) -> Self {
        // Corruption/device errors must not be hidden behind another mount.
        // Wrong content decryption instead fails UTF-8/layout validation.
        if error.chain().any(|cause| cause.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() != std::io::ErrorKind::InvalidData || io.raw_os_error().is_some()))
        {
            Self::Service(error)
        } else {
            Self::Contents(error)
        }
    }
}

pub(crate) fn options(original: &[String], inline: bool) -> Vec<String> {
    let mut out: Vec<_> = original.iter().filter(|word| word.as_str() != "inlinecrypt").cloned().collect();
    if inline { out.push("inlinecrypt".into()); }
    out
}

/// Each verification is on an owned, read-only mount. Only PRIVATE/ICE v1
/// permits an alternate. Stop on I/O, service, or unmount/mount errors; no
/// user data is exposed until the key files authenticate successfully.
pub(crate) fn probe(
    initial: bool,
    alternate: bool,
    mut switch: impl FnMut(bool) -> Result<()>,
    mut verify: impl FnMut() -> std::result::Result<(), ProbeError>,
) -> Result<bool> {
    switch(initial).context("prepare initial read-only legacy data probe")?;
    let first = match verify() {
        Ok(()) => return Ok(initial),
        Err(ProbeError::Service(error)) => return Err(error).context("legacy data probe service"),
        Err(ProbeError::Contents(error)) if !alternate => return Err(error).context("legacy data probe contents"),
        Err(ProbeError::Contents(error)) => error,
    };
    switch(!initial).with_context(|| format!("switch read-only legacy data mount after: {first:#}"))?;
    match verify() {
        Ok(()) => Ok(!initial),
        Err(ProbeError::Contents(error) | ProbeError::Service(error)) => Err(error)
            .context(format!("both read-only legacy data probes failed; inlinecrypt={initial}: {first:#}; inlinecrypt={}", !initial)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn current_mount_is_kept_when_the_de_key_authenticates() {
        let switches = RefCell::new(Vec::new());
        assert!(!probe(false, true, |inline| { switches.borrow_mut().push(inline); Ok(()) }, || Ok(())).unwrap());
        assert_eq!(*switches.borrow(), [false]);
    }

    #[test]
    fn private_data_can_select_the_other_path_before_writes() {
        let switches = RefCell::new(Vec::new());
        let count = Cell::new(0);
        let selected = probe(false, true, |inline| { switches.borrow_mut().push(inline); Ok(()) }, || {
            count.set(count.get() + 1);
            if count.get() == 1 { Err(ProbeError::contents(anyhow::anyhow!("invalid key-file layout"))) } else { Ok(()) }
        }).unwrap();
        assert!(selected);
        assert_eq!(*switches.borrow(), [false, true]);
        assert_eq!(count.get(), 2);
        // Writable transition uses the same selected option.
        let selected = options(&["nosuid".into(), "nodev".into(), "data=ordered".into()], selected);
        for writable in [false, true] {
            let mounted = crate::plain::mount_options(&selected, writable);
            assert!(mounted.iter().any(|s| s == "inlinecrypt"));
            assert!(mounted.iter().any(|s| s == "nosuid"));
            assert!(mounted.iter().any(|s| s == "nodev"));
            assert!(!mounted.iter().any(|s| s == "noload" || s == "norecovery"));
        }
    }

    #[test]
    fn service_and_filesystem_errors_never_trigger_an_alternate() {
        for error in [
            ProbeError::Service(anyhow::anyhow!("Keymaster rejected key")),
            ProbeError::contents(std::io::Error::from_raw_os_error(5).into()),
            ProbeError::contents(std::io::Error::from_raw_os_error(117).into()),
        ] {
            let count = Cell::new(0);
            let mut error = Some(error);
            assert!(probe(false, true, |_| { count.set(count.get() + 1); Ok(()) },
                || Err(error.take().expect("no retry"))).is_err());
            assert_eq!(count.get(), 1);
        }
    }

    #[test]
    fn standard_modes_do_not_fall_back_and_two_failures_stay_visible() {
        let count = Cell::new(0);
        assert!(probe(false, false, |_| { count.set(count.get()+1); Ok(()) },
            || Err(ProbeError::contents(anyhow::anyhow!("bad layout")))).is_err());
        assert_eq!(count.get(), 1);
        let error = probe(false, true, |_| Ok(()),
            || Err(ProbeError::contents(anyhow::anyhow!("bad layout")))).unwrap_err();
        assert!(format!("{error:#}").contains("both read-only legacy data probes failed"));
    }

    #[test]
    fn a_failed_switch_stops_before_another_read() {
        let count = Cell::new(0);
        let error = probe(false, true, |inline| {
            if inline { anyhow::bail!("mount busy") } else { Ok(()) }
        }, || {
            count.set(count.get() + 1);
            Err(ProbeError::contents(anyhow::anyhow!("bad layout")))
        }).unwrap_err();
        assert_eq!(count.get(), 1);
        assert!(format!("{error:#}").contains("mount busy"));
    }

    #[test]
    fn only_inlinecrypt_changes() {
        let initial = ["nosuid", "nodev", "noatime", "data=ordered", "noauto_da_alloc", "inlinecrypt", "inlinecrypt"]
            .into_iter().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(options(&initial, false), ["nosuid", "nodev", "noatime", "data=ordered", "noauto_da_alloc"]);
        assert_eq!(options(&initial, true), ["nosuid", "nodev", "noatime", "data=ordered", "noauto_da_alloc", "inlinecrypt"]);
    }
}
