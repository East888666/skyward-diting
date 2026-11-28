//! Optional AWK in the ramdisk, available after installed partitions detach.

use anyhow::{ensure, Context, Result};
use skyward_bootimg::Ramdisk;
use skyward_config::ramdisk as paths;
use std::path::Path;

const BINARY: &[u8] = include_bytes!("../assets/awk/awk-aarch64-linux-android");
const LICENSE: &[u8] = include_bytes!("../assets/awk/LICENSE");
const VERSION: &[u8] = include_bytes!("../assets/awk/VERSION");

pub fn install_selected(rd: &mut Ramdisk, enabled: bool, bundle: Option<&Path>) -> Result<()> {
    if let Some(bundle) = bundle {
        install(rd, bundle)
    } else if enabled {
        install_bytes(rd, BINARY, LICENSE, VERSION)
    } else {
        Ok(())
    }
}

pub fn install(rd: &mut Ramdisk, bundle: &Path) -> Result<()> {
    let read = |name: &str| std::fs::read(bundle.join(name))
        .with_context(|| format!("read AWK bundle {}", bundle.join(name).display()));
    // Read and validate the complete bundle before changing the archive.
    let binary = read("awk")?;
    let license = read("LICENSE")?;
    let version = read("VERSION")?;
    install_bytes(rd, &binary, &license, &version)
}

fn install_bytes(rd: &mut Ramdisk, binary: &[u8], license: &[u8], version: &[u8]) -> Result<()> {
    super::recovery::validate_static_program(binary, "AWK")?;
    ensure!(!license.is_empty() && !version.is_empty(), "AWK requires nonempty LICENSE and VERSION");
    let recovery = [paths::RECOVERY, "sbin/recovery"].into_iter()
        .find(|path| rd.find(path).is_some()).context("AWK requires a recovery binary")?;
    let directory = recovery.rsplit_once('/').unwrap().0;
    let command = format!("{directory}/awk");
    if let Some(entry) = rd.find(&command) {
        ensure!(matches!(entry.mode & 0o170000, 0o100000 | 0o120000),
            "AWK cannot replace /{command}: not a file or link");
    }
    // The config or CLI explicitly selects AWK; never change toybox itself.
    rd.drop_at(&command);
    rd.add(&command, 0o755, binary.to_vec())?;
    rd.add("skyward/awk-LICENSE", 0o644, license.to_vec())?;
    rd.add("skyward/awk-VERSION", 0o644, version.to_vec())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn bundled_awk_is_the_verified_static_binary_and_config_controls_inclusion() {
        assert_eq!(hex::encode(Sha256::digest(BINARY)),
            "2663b5f150ee14da50def2a83f5f6f8f9df03ba649861df3b392e3c26f1dc50a");
        super::super::recovery::validate_static_program(BINARY, "bundled AWK").unwrap();
        let mut rd = Ramdisk { entries: Vec::new() };
        rd.add("system/bin/recovery", 0o755, vec![]).unwrap();
        install_selected(&mut rd, false, None).unwrap();
        assert!(rd.find("system/bin/awk").is_none());
        assert!(rd.find("skyward/awk-LICENSE").is_none());
        install_selected(&mut rd, true, None).unwrap();
        let awk = rd.find("system/bin/awk").unwrap();
        assert_eq!(awk.data, BINARY);
        assert_eq!(awk.mode, 0o100755);
        assert_eq!(rd.find("skyward/awk-LICENSE").unwrap().data, LICENSE);
        assert_eq!(rd.find("skyward/awk-LICENSE").unwrap().mode, 0o100644);
        assert_eq!(rd.find("skyward/awk-VERSION").unwrap().data, VERSION);
    }

    #[test]
    fn explicit_bundle_overrides_embedded_awk_even_when_config_is_disabled() {
        let bundle = bundle();
        for enabled in [false, true] {
            let mut rd = Ramdisk { entries: Vec::new() };
            rd.add("sbin/recovery", 0o755, vec![]).unwrap();
            install_selected(&mut rd, enabled, Some(bundle.path())).unwrap();
            assert_eq!(rd.find("sbin/awk").unwrap().data, executable());
        }
    }

    fn executable() -> Vec<u8> {
        let mut data = vec![0; 120];
        data[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        data[16..18].copy_from_slice(&2u16.to_le_bytes());
        data[18..20].copy_from_slice(&183u16.to_le_bytes());
        data[20..24].copy_from_slice(&1u32.to_le_bytes());
        data[24..32].copy_from_slice(&0x400000u64.to_le_bytes());
        data[32..40].copy_from_slice(&64u64.to_le_bytes());
        data[52..54].copy_from_slice(&64u16.to_le_bytes());
        data[54..56].copy_from_slice(&56u16.to_le_bytes());
        data[56..58].copy_from_slice(&1u16.to_le_bytes());
        data[64..68].copy_from_slice(&1u32.to_le_bytes());
        data[68..72].copy_from_slice(&5u32.to_le_bytes());
        data[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
        data[96..104].copy_from_slice(&120u64.to_le_bytes());
        data[104..112].copy_from_slice(&120u64.to_le_bytes());
        data
    }

    fn bundle() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, data) in [("awk", executable()), ("LICENSE", b"permission".to_vec()), ("VERSION", b"version".to_vec())] {
            std::fs::write(dir.path().join(name), data).unwrap();
        }
        dir
    }

    #[test]
    fn static_awk_and_license_are_kept_in_both_recovery_layouts() {
        let bundle = bundle();
        for directory in ["system/bin", "sbin"] {
            let mut rd = Ramdisk { entries: Vec::new() };
            rd.add(&format!("{directory}/recovery"), 0o755, b"recovery".to_vec()).unwrap();
            rd.add(&format!("{directory}/toybox"), 0o755, b"stock".to_vec()).unwrap();
            rd.link(&format!("{directory}/awk"), "toybox").unwrap();
            install(&mut rd, bundle.path()).unwrap();
            let awk = rd.find(&format!("{directory}/awk")).unwrap();
            assert_eq!(awk.mode, 0o100755);
            assert_eq!(awk.data, executable());
            assert_eq!(rd.find(&format!("{directory}/toybox")).unwrap().data, b"stock");
            assert_eq!(rd.find("skyward/awk-LICENSE").unwrap().data, b"permission");
            assert_eq!(rd.find("skyward/awk-VERSION").unwrap().data, b"version");
        }
    }

    #[test]
    fn invalid_or_incomplete_bundles_do_not_replace_stock_awk() {
        for failure in ["architecture", "dynamic", "license"] {
            let bundle = bundle();
            if failure == "license" {
                std::fs::remove_file(bundle.path().join("LICENSE")).unwrap();
            } else {
                let mut binary = executable();
                if failure == "architecture" { binary[18..20].copy_from_slice(&62u16.to_le_bytes()); }
                else { binary[64..68].copy_from_slice(&3u32.to_le_bytes()); }
                std::fs::write(bundle.path().join("awk"), binary).unwrap();
            }
            let mut rd = Ramdisk { entries: Vec::new() };
            rd.add("system/bin/recovery", 0o755, vec![]).unwrap();
            rd.add("system/bin/awk", 0o755, b"stock awk".to_vec()).unwrap();
            assert!(install(&mut rd, bundle.path()).is_err());
            assert_eq!(rd.find("system/bin/awk").unwrap().data, b"stock awk");
            assert!(rd.find("skyward/awk-LICENSE").is_none());
        }
    }
}
