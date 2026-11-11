//! Inspect a directly mounted filesystem before writing shared storage.

use anyhow::{ensure, Context, Result};
use std::fs;
use std::path::Path;

/// Inspect a consistent filesystem read-only, then enable ordinary writes.
/// A read-only ext4 mount still replays committed journal transactions.
/// noload/norecovery must never leak into either phase: directory lookup
/// before replay can report EUCLEAN, and remounting that view writable
/// leaves journaling disabled. Keep the table's security/driver options.
pub(crate) fn mount_options(options: &[String], writable: bool) -> Vec<String> {
    let mut out: Vec<String> = options.iter()
        .filter(|o| !matches!(o.as_str(), "ro" | "rw" | "noload" | "norecovery"))
        .cloned().collect();
    out.push(if writable { "rw" } else { "ro" }.into());
    out
}

/// True only for an empty, policy-free filesystem needing media/0. A table
/// can request FBE for the next Android boot without a freshly made
/// filesystem already having keys or policies. Never infer this merely
/// from missing keys or missing media on a populated filesystem.
pub(crate) fn inspect(root: &Path, file_encryption: bool, policy_free: impl Fn(&Path) -> Result<()>) -> Result<bool> {
    for marker in ["unencrypted", "misc/vold/user_keys", "unencrypted_v2"] {
        ensure!(!present(&root.join(marker))?, "{marker} indicates file encryption; data is not decrypted");
    }
    for name in ["", "media", "media/0", "user", "user/0", "user_de/0", "system_de/0"] {
        let path = root.join(name);
        if present(&path)? {
            ensure!(fs::symlink_metadata(&path)?.is_dir(), "{} is not a directory", path.display());
            policy_free(&path)?;
        }
    }
    let media = present(&root.join("media"))?;
    if media && !file_encryption {
        return Ok(false);
    }
    for entry in fs::read_dir(root).context("inspect freshly formatted data")? {
        let entry = entry?;
        // A previously initialized fresh filesystem can contain files
        // copied into its policy-free media directory before Android boots.
        if media && entry.file_name() == "media" { continue; }
        ensure!(entry.file_name() == "lost+found" && entry.file_type()?.is_dir(),
            "data has existing contents but no shared storage; refusing to create it");
        policy_free(&entry.path())?;
        ensure!(fs::read_dir(entry.path())?.next().transpose()?.is_none(),
            "data contains recovered files; refusing to initialize shared storage");
    }
    Ok(!media)
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("inspect {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn data_mounts_replay_the_journal_before_inspection_and_keep_security_options() {
        let options: Vec<String> = ["ro", "noload", "nosuid", "nodev", "noatime", "norecovery", "rw", "data=ordered", "noauto_da_alloc"]
            .into_iter().map(str::to_string).collect();
        for writable in [false, true] {
            let expected = ["nosuid", "nodev", "noatime", "data=ordered", "noauto_da_alloc", if writable { "rw" } else { "ro" }];
            assert_eq!(mount_options(&options, writable), expected);
        }
    }

    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!("skyward-plain-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn inspect(&self, encryption: bool) -> Result<bool> { inspect(&self.0, encryption, |_| Ok(())) }
    }
    impl Drop for Temp {
        fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); }
    }

    #[test]
    fn fresh_filesystems_can_initialize_even_if_android_will_encrypt_them() {
        let temp = Temp::new();
        assert!(temp.inspect(false).unwrap());
        assert!(temp.inspect(true).unwrap());
        fs::create_dir(temp.0.join("lost+found")).unwrap();
        assert!(temp.inspect(true).unwrap());
        assert!(!temp.0.join("media").exists(), "inspection must not write");
    }

    #[test]
    fn missing_media_on_populated_data_is_not_a_fresh_format() {
        for name in ["user", "misc", "unknown"] {
            let temp = Temp::new();
            fs::create_dir(temp.0.join(name)).unwrap();
            assert!(temp.inspect(false).is_err());
        }
        let temp = Temp::new();
        fs::create_dir(temp.0.join("lost+found")).unwrap();
        fs::write(temp.0.join("lost+found/recovered"), b"data").unwrap();
        assert!(temp.inspect(false).is_err());
    }

    #[test]
    fn encryption_markers_and_policies_are_never_treated_as_plain() {
        for marker in ["unencrypted", "misc/vold/user_keys", "unencrypted_v2"] {
            let temp = Temp::new();
            fs::create_dir_all(temp.0.join(marker)).unwrap();
            assert!(temp.inspect(false).is_err());
        }
        let temp = Temp::new();
        assert!(inspect(&temp.0, false, |_| anyhow::bail!("encrypted policy")).is_err());
        fs::create_dir_all(temp.0.join("media/0")).unwrap();
        assert!(inspect(&temp.0, false, |path| {
            ensure!(path != temp.0.join("media/0"), "encrypted media");
            Ok(())
        }).is_err());
    }

    #[test]
    fn initialized_plain_media_can_be_reopened_before_android_creates_keys() {
        let temp = Temp::new();
        fs::create_dir_all(temp.0.join("media/0")).unwrap();
        fs::write(temp.0.join("media/0/keep"), b"keep").unwrap();
        assert!(!temp.inspect(false).unwrap());
        assert!(!temp.inspect(true).unwrap());
        fs::create_dir(temp.0.join("system_ce")).unwrap();
        assert!(temp.inspect(true).is_err());
        assert_eq!(fs::read(temp.0.join("media/0/keep")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_disguise_missing_markers_or_fresh_directories() {
        for name in ["unencrypted", "media", "lost+found"] {
            let temp = Temp::new();
            std::os::unix::fs::symlink("absent", temp.0.join(name)).unwrap();
            assert!(temp.inspect(false).is_err());
        }
    }
}
