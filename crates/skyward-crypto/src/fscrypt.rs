//! Letting a key into a filesystem, so the files it was used on can be
//! read. The kernel keeps the key for the filesystem, not for any one
//! file, and names it by a hash it works out itself.

use anyhow::{bail, Context, Result};
use std::path::Path;

/// How long the name the kernel gives a key is.
pub const NAME_BYTES: usize = 16;

/// The fscrypt policy API, independently of the Keymaster HAL version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileEncryptionMode { V1, V2 }

pub fn file_encryption_mode(mode: &str) -> Result<FileEncryptionMode> {
    // Wrapped keys require an ephemeral-key operation absent in Keymaster 3.
    anyhow::ensure!(!hardware_wrapped(mode), "Keymaster 3 cannot unwrap hardware-wrapped fscrypt keys");
    match mode.trim() {
        "ice:aes-256-cts:v1" | "aes-256-xts:aes-256-cts:v1" => Ok(FileEncryptionMode::V1),
        "aes-256-xts:aes-256-cts:v2" => Ok(FileEncryptionMode::V2),
        _ => bail!("unsupported Keymaster 3 file encryption mode {mode:?}"),
    }
}

impl FileEncryptionMode {
    pub fn reference_bytes(self) -> usize {
        match self { Self::V1 => 8, Self::V2 => NAME_BYTES }
    }

    pub fn validate_reference(self, reference: &[u8]) -> Result<()> {
        anyhow::ensure!(reference.len() == self.reference_bytes(),
            "{self:?} common key needs a {}-byte reference, found {}", self.reference_bytes(), reference.len());
        Ok(())
    }

    #[cfg(target_os = "android")]
    pub fn install(self, mount: &Path, raw: &[u8], expected: Option<&[u8]>) -> Result<()> {
        if let Some(expected) = expected { self.validate_reference(expected)?; }
        match self {
            Self::V1 => legacy_add_key(raw, expected.map(|r| r.try_into().expect("validated descriptor"))).map(|_| ()),
            Self::V2 => {
                anyhow::ensure!(raw.len() == 64, "AES-256-XTS key must be 64 bytes");
                let identifier = add_key(mount, raw, false)?;
                anyhow::ensure!(expected.is_none_or(|expected| expected == identifier), "fscrypt v2 key differs from its recorded identifier");
                Ok(())
            }
        }
    }

    #[cfg(target_os = "android")]
    pub fn unlocked(self, dir: &Path) -> Result<bool> {
        match self { Self::V1 => legacy_unlocked(dir), Self::V2 => unlocked(dir) }
    }
}

/// Let `raw` into the filesystem mounted at `mount`, and give the name the
/// kernel knows it by. Every file whose directory names that key can be
/// read from then on.
#[cfg(target_os = "android")]
pub fn add_key(mount: &Path, raw: &[u8], wrapped: bool) -> Result<[u8; NAME_BYTES]> {
    use std::os::fd::AsRawFd;
    /// The request's fixed part, before the key itself.
    const HEADER_BYTES: usize = 80;
    /// The key is named by what the kernel makes of it rather than by
    /// anything this side chooses.
    const BY_IDENTIFIER: u32 = 2;
    /// The key is one only the storage hardware can unwrap.
    const HARDWARE_WRAPPED: u32 = 1;
    const ADD_KEY: u32 = 0xc050_6617;

    let mut arg = vec![0u8; HEADER_BYTES + raw.len()];
    arg[0..4].copy_from_slice(&BY_IDENTIFIER.to_ne_bytes());
    arg[40..44].copy_from_slice(&(raw.len() as u32).to_ne_bytes());
    if wrapped {
        arg[76..80].copy_from_slice(&HARDWARE_WRAPPED.to_ne_bytes());
    }
    arg[HEADER_BYTES..].copy_from_slice(raw);
    let installed = (|| -> Result<()> {
        let dir = std::fs::File::open(mount).with_context(|| format!("open {}", mount.display()))?;
        if unsafe { libc::ioctl(dir.as_raw_fd(), ADD_KEY as libc::Ioctl, arg.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| format!("let a key into {}", mount.display()));
        }
        Ok(())
    })();
    arg[HEADER_BYTES..].fill(0);
    installed?;
    let mut name = [0u8; NAME_BYTES];
    name.copy_from_slice(&arg[8..8 + NAME_BYTES]);
    Ok(name)
}

/// Whether the names and contents under `dir` read as they are: it is not
/// encrypted, or the key it is encrypted with is in the kernel. A user's
/// own files read only once that user's key is let in.
#[cfg(target_os = "android")]
pub fn unlocked(dir: &Path) -> Result<bool> {
    use std::os::fd::AsRawFd;
    const GET_POLICY: u32 = 0xc009_6616;
    const GET_KEY_STATUS: u32 = 0xc080_661a;
    /// How the first kind of policy names its key, and how the second does.
    const BY_DESCRIPTOR: u32 = 1;
    const BY_IDENTIFIER: u32 = 2;
    /// The second kind of policy is the largest: its version, three bytes
    /// of how it encrypts, four reserved, and the key's name.
    const POLICY_BYTES: usize = 24;
    const PRESENT: u32 = 2;

    let folder = std::fs::File::open(dir).with_context(|| format!("open {}", dir.display()))?;
    let mut policy = [0u8; 8 + POLICY_BYTES];
    policy[..8].copy_from_slice(&(POLICY_BYTES as u64).to_ne_bytes());
    if unsafe { libc::ioctl(folder.as_raw_fd(), GET_POLICY as libc::Ioctl, policy.as_mut_ptr()) } != 0 {
        let e = std::io::Error::last_os_error();
        // The folder is not encrypted, or its filesystem was made without
        // the means to encrypt any, as one that holds a partition the
        // phone keeps plain may be.
        if matches!(e.raw_os_error(), Some(libc::ENODATA | libc::EOPNOTSUPP)) {
            return Ok(true);
        }
        return Err(e).with_context(|| format!("read how {} is encrypted", dir.display()));
    }
    let mut status = [0u8; 128];
    match policy[8] {
        0 => {
            status[..4].copy_from_slice(&BY_DESCRIPTOR.to_ne_bytes());
            status[8..16].copy_from_slice(&policy[12..20]);
        }
        2 => {
            status[..4].copy_from_slice(&BY_IDENTIFIER.to_ne_bytes());
            status[8..8 + NAME_BYTES].copy_from_slice(&policy[16..16 + NAME_BYTES]);
        }
        other => bail!("{} is encrypted under a policy of version {other}, which is not known here", dir.display()),
    }
    if unsafe { libc::ioctl(folder.as_raw_fd(), GET_KEY_STATUS as libc::Ioctl, status.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("ask whether the key for {} is in", dir.display()));
    }
    Ok(u32::from_ne_bytes(status[64..68].try_into().expect("four bytes")) == PRESENT)
}

/// Whether what a phone says about how it encrypts its files means the
/// keys are ones only the storage hardware can unwrap.
pub fn hardware_wrapped(mode: &str) -> bool {
    mode.split(['+', ':']).any(|part| part == "wrappedkey_v0")
}

/// Read the name the phone recorded for a key it let in, to check against
/// the name the kernel works out now.
pub fn recorded_name(path: &Path) -> Result<[u8; NAME_BYTES]> {
    let raw = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    match <[u8; NAME_BYTES]>::try_from(&raw[..]) {
        Ok(name) => Ok(name),
        Err(_) => bail!("{} holds {} bytes, and a key's name is {NAME_BYTES}", path.display(), raw.len()),
    }
}

/// Old v1 policies identify their 64-byte key by a double SHA-512 hash.
/// This is the vold KeyUtil algorithm, not the modern kernel identifier.
pub fn legacy_descriptor(raw: &[u8]) -> Result<[u8; 8]> {
    use sha2::{Digest, Sha512};
    anyhow::ensure!(raw.len() == 64, "legacy fscrypt key must be 64 bytes");
    Ok(Sha512::digest(Sha512::digest(raw))[..8].try_into().expect("eight bytes"))
}

#[cfg(target_os = "android")]
pub fn legacy_prepare_keyring() -> Result<i32> {
    use std::ffi::CString;
    const SESSION: libc::c_long = -3;
    let session = unsafe { libc::syscall(libc::SYS_keyctl, 0, SESSION, 1, 0, 0) };
    if session < 0 { return Err(std::io::Error::last_os_error()).context("open recovery session keyring"); }
    let kind = CString::new("keyring")?;
    let name = CString::new("fscrypt")?;
    let found = unsafe { libc::syscall(libc::SYS_keyctl, 10, session, kind.as_ptr(), name.as_ptr(), 0) };
    if found >= 0 { return Ok(found as i32); }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() != Some(libc::ENOKEY) { return Err(error).context("find fscrypt keyring"); }
    let created = unsafe { libc::syscall(libc::SYS_add_key, kind.as_ptr(), name.as_ptr(), std::ptr::null::<u8>(), 0, session) };
    if created < 0 { return Err(std::io::Error::last_os_error()).context("create fscrypt session keyring"); }
    Ok(created as i32)
}

#[cfg(target_os = "android")]
pub fn legacy_add_key(raw: &[u8], expected: Option<&[u8; 8]>) -> Result<[u8; 8]> {
    use std::ffi::CString;
    let descriptor = legacy_descriptor(raw)?;
    anyhow::ensure!(expected.is_none_or(|expected| expected == &descriptor), "legacy key differs from its recorded descriptor");
    let ring = legacy_prepare_keyring()?;
    let mut payload = [0u8; 72];
    payload[..4].copy_from_slice(&1u32.to_ne_bytes()); // FS_ENCRYPTION_MODE_AES_256_XTS
    payload[4..68].copy_from_slice(raw);
    payload[68..].copy_from_slice(&64u32.to_ne_bytes());
    let kind = CString::new("logon")?;
    let suffix: String = descriptor.iter().map(|b| format!("{b:02x}")).collect();
    let result = (|| -> Result<()> {
        for prefix in ["ext4", "f2fs", "fscrypt"] {
            let name = CString::new(format!("{prefix}:{suffix}"))?;
            let added = unsafe { libc::syscall(libc::SYS_add_key, kind.as_ptr(), name.as_ptr(), payload.as_ptr(), payload.len(), ring) };
            if added < 0 { return Err(std::io::Error::last_os_error()).context("install legacy fscrypt key"); }
        }
        Ok(())
    })();
    payload.fill(0);
    result?;
    Ok(descriptor)
}

/// Confirm the exact policy descriptor has a logon key in this session.
/// Old kernels lack FS_IOC_GET_ENCRYPTION_KEY_STATUS; do not send that
/// modern ioctl or mistake ENOTTY for a decrypted directory.
#[cfg(target_os = "android")]
pub fn legacy_unlocked(dir: &Path) -> Result<bool> {
    use std::{ffi::CString, os::fd::AsRawFd};
    let folder = std::fs::File::open(dir).with_context(|| format!("open {}", dir.display()))?;
    let mut policy = [0u8; 12];
    if unsafe { libc::ioctl(folder.as_raw_fd(), 0x400c6615u32 as libc::Ioctl, policy.as_mut_ptr()) } != 0 {
        let error = std::io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::ENODATA | libc::ENOENT)) {
            return Err(error).with_context(|| format!("read legacy encryption policy of {}", dir.display()));
        }
        // Downstream ext4 can report ENOENT instead of ENODATA for an
        // inode without a policy. Inspect this same open inode; a missing
        // context on an encrypted inode must still fail.
        const GET_FLAGS: u32 = 0x8000_6601 | ((std::mem::size_of::<libc::c_long>() as u32) << 16);
        let mut flags: libc::c_long = 0;
        if unsafe { libc::ioctl(folder.as_raw_fd(), GET_FLAGS as libc::Ioctl, &mut flags) } != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("read inode flags of {} after missing encryption policy", dir.display()));
        }
        return crate::plain::ensure_unencrypted_flags(flags as u32).map(|()| true)
            .with_context(|| format!("inspect legacy encryption of {}", dir.display()));
    }
    anyhow::ensure!(policy[0] == 0 && matches!(policy[1], 1 | 127) && policy[2] == 4, "unsupported legacy encryption policy");
    let name: String = policy[4..12].iter().map(|b| format!("{b:02x}")).collect();
    let kind = CString::new("logon")?;
    let name = CString::new(format!("fscrypt:{name}"))?;
    let ring = legacy_prepare_keyring()?;
    let key = unsafe { libc::syscall(libc::SYS_keyctl, 10, ring, kind.as_ptr(), name.as_ptr(), 0) };
    if key >= 0 { return Ok(true); }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOKEY) { return Ok(false); }
    Err(error).context("find key for legacy encryption policy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymaster_version_does_not_determine_the_fscrypt_policy_version() {
        assert_eq!(file_encryption_mode("ice:aes-256-cts:v1").unwrap(), FileEncryptionMode::V1);
        assert_eq!(file_encryption_mode("aes-256-xts:aes-256-cts:v1").unwrap(), FileEncryptionMode::V1);
        let v2 = file_encryption_mode("aes-256-xts:aes-256-cts:v2").unwrap();
        assert_eq!(v2, FileEncryptionMode::V2);
        assert!(v2.validate_reference(&[0; 16]).is_ok());
        assert!(v2.validate_reference(&[0; 8]).is_err());
        assert!(FileEncryptionMode::V1.validate_reference(&[0; 16]).is_err());
        assert!(file_encryption_mode("ice:aes-256-cts:v2").is_err());
        assert!(file_encryption_mode("aes-256-xts:aes-256-cts:v3").is_err());
        assert!(file_encryption_mode("aes-256-xts:aes-256-cts:v2+wrappedkey_v0").is_err());
    }

    #[test]
    fn legacy_descriptor_matches_an_independent_double_sha512_vector() {
        let descriptor = legacy_descriptor(&[0u8; 64]).unwrap();
        assert_eq!(descriptor, [245, 116, 167, 116, 100, 1, 115, 17]);
        assert!(legacy_descriptor(&[0; 32]).is_err());
    }

    #[test]
    fn a_phone_says_whether_the_hardware_holds_the_keys() {
        assert!(hardware_wrapped("aes-256-xts:aes-256-cts:v2+inlinecrypt_optimized+wrappedkey_v0"));
        assert!(!hardware_wrapped("aes-256-xts:aes-256-cts:v2"));
        assert!(!hardware_wrapped("aes-256-xts:aes-256-cts:v2+wrappedkey_v1"));
    }

    #[test]
    fn a_name_is_read_back_only_at_its_own_length() {
        let dir = std::env::temp_dir().join(format!("skyward-fscrypt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("ref");
        std::fs::write(&good, [7u8; NAME_BYTES]).unwrap();
        assert_eq!(recorded_name(&good).unwrap(), [7u8; NAME_BYTES]);
        let short = dir.join("short");
        std::fs::write(&short, [7u8; 8]).unwrap();
        assert!(recorded_name(&short).is_err());
    }
}
