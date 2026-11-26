//! Read-only access to Android 11's file-backed keystore. Only plain
//! hardware key containers are accepted; encrypted, software and StrongBox
//! containers need their own implementations and must not reach Keymaster 3.

use anyhow::{ensure, Context, Result};
use std::path::Path;

pub fn key_blob(root: &Path, alias: &str) -> Result<Vec<u8>> {
    ensure!(!alias.is_empty() && alias.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'), "invalid legacy keystore alias");
    // LockSettingsService owns all synthetic-password keys as system UID,
    // including protectors belonging to secondary Android users.
    let path = root.join(format!("user_0/1000_USRPKEY_{alias}"));
    let raw = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    parse(&raw).with_context(|| format!("read legacy hardware key in {}", path.display()))
}

fn parse(raw: &[u8]) -> Result<Vec<u8>> {
    const HEADER: usize = 40;
    ensure!(raw.len() >= HEADER, "legacy keystore container is truncated");
    ensure!(raw[0] == 3 && raw[1] == 4, "unsupported legacy keystore version or key type");
    // KEYSTORE_FLAG_CRITICAL_TO_DEVICE_ENCRYPTION (8) is a storage flag,
    // not another encryption layer. Reject every other flag here.
    ensure!(raw[2] & !8 == 0, "legacy keystore key is encrypted or is not a normal hardware key");
    let size = u32::from_be_bytes(raw[36..40].try_into().expect("four bytes")) as usize;
    ensure!(size > 0 && HEADER.checked_add(size).and_then(|n| n.checked_add(raw[3] as usize)) == Some(raw.len()),
        "legacy keystore container length is invalid");
    Ok(raw[HEADER..HEADER + size].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(flags: u8) -> Vec<u8> {
        let mut raw = vec![0; 40];
        raw[..4].copy_from_slice(&[3, 4, flags, 2]);
        raw[36..40].copy_from_slice(&3u32.to_be_bytes());
        raw.extend([1, 2, 3, 9, 9]);
        raw
    }

    #[test]
    fn extracts_hardware_blob_without_the_header_or_info() {
        assert_eq!(parse(&container(8)).unwrap(), [1, 2, 3]);
        assert_eq!(parse(&container(0)).unwrap(), [1, 2, 3]);
    }

    #[test]
    fn refuses_encrypted_other_hardware_and_malformed_containers() {
        for flag in [1, 2, 4, 16, 128] { assert!(parse(&container(flag)).is_err()); }
        let raw = container(8);
        for n in 0..raw.len() { assert!(parse(&raw[..n]).is_err()); }
        let mut raw = raw;
        raw[0] = 2;
        assert!(parse(&raw).is_err());
        raw[0] = 3;
        raw[1] = 1;
        assert!(parse(&raw).is_err());
        assert!(key_blob(Path::new("/does-not-exist"), "../other").is_err());
    }
}
