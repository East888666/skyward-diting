use anyhow::{Context, Result};
use skyward_bootimg::Ramdisk;
use skyward_config::ramdisk as paths;

const SHELL: &str = "system/bin/sh";
const STOCK_SHELLS: [&str; 1] = ["sbin/sh"];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_AARCH64: u16 = 183;
const PT_INTERP: u32 = 3;

/// Ensure scripts can execute `/system/bin/sh`, using only a shell already in
/// the stock ramdisk or a caller-provided static ARM64 ELF.
pub fn install_shell(rd: &mut Ramdisk, shell: Option<Vec<u8>>) -> Result<()> {
    if rd.find(SHELL).is_some() {
        return Ok(());
    }
    for candidate in STOCK_SHELLS {
        if rd
            .find(candidate)
            .is_some_and(|e| e.mode & 0o170000 == 0o100000 && e.mode & 0o111 != 0)
        {
            rd.link(SHELL, &format!("/{candidate}"))?;
            return Ok(());
        }
    }
    let Some(shell) = shell else {
        eprintln!("warning: no recovery shell found; adb shell requires --shell with a static ARM64 shell (BusyBox must include ash)");
        return Ok(());
    };
    validate_shell(&shell)?;
    rd.add(SHELL, 0o755, shell)
}

fn validate_shell(data: &[u8]) -> Result<()> {
    anyhow::ensure!(data.len() >= 64, "--shell is shorter than an ELF64 header");
    anyhow::ensure!(&data[..4] == b"\x7fELF", "--shell is not an ELF file");
    anyhow::ensure!(data[4] == ELFCLASS64, "--shell must be a 64-bit ELF");
    anyhow::ensure!(data[5] == ELFDATA2LSB, "--shell must be little-endian");
    let kind = elf_u16(data, 16)?;
    anyhow::ensure!(kind == ET_EXEC || kind == ET_DYN, "--shell must be ET_EXEC or ET_DYN");
    anyhow::ensure!(elf_u16(data, 18)? == EM_AARCH64, "--shell must target AArch64");
    let phoff = usize::try_from(elf_u64(data, 32)?)?;
    let phentsize = usize::from(elf_u16(data, 54)?);
    let phnum = usize::from(elf_u16(data, 56)?);
    anyhow::ensure!(phentsize >= 56, "--shell has invalid ELF64 program-header size");
    let phdrs = phentsize.checked_mul(phnum).context("--shell program headers overflow")?;
    let end = phoff.checked_add(phdrs).context("--shell program headers overflow")?;
    anyhow::ensure!(end <= data.len(), "--shell program headers are outside the file");
    for header in data[phoff..end].chunks_exact(phentsize) {
        anyhow::ensure!(u32::from_le_bytes(header[..4].try_into().unwrap()) != PT_INTERP,
            "--shell has PT_INTERP and is dynamically linked");
    }
    Ok(())
}

fn elf_u16(data: &[u8], at: usize) -> Result<u16> {
    let bytes = data.get(at..at + 2).context("--shell ELF header is truncated")?;
    Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn elf_u64(data: &[u8], at: usize) -> Result<u64> {
    let bytes = data.get(at..at + 8).context("--shell ELF header is truncated")?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

/// 仅显式导入时替换 init 已声明的 adbd；不修改服务参数、USB 动作或策略。
pub fn install_adbd(rd: &mut Ramdisk, binary: Vec<u8>) -> Result<String> {
    validate_static_adbd(&binary)?;
    let mut services = Vec::new();
    for entry in &rd.entries {
        if entry.name.ends_with(".rc") && entry.mode & 0o170000 == 0o100000 {
            let text = std::str::from_utf8(&entry.data).context("--adbd init rc is not UTF-8")?;
            for line in text.lines() {
                let words: Vec<_> = line.split('#').next().unwrap_or_default().split_whitespace().collect();
                if words.first() == Some(&"service") && words.get(1) == Some(&"adbd") {
                    let path = *words.get(2).context("--adbd service has no executable")?;
                    anyhow::ensure!(matches!(path, "/sbin/adbd" | "/system/bin/adbd"),
                        "--adbd unsupported stock service path: {path}");
                    services.push(path.trim_start_matches('/').to_owned());
                }
            }
        }
    }
    anyhow::ensure!(services.len() == 1, "--adbd requires exactly one stock service adbd definition, found {}", services.len());
    let path = services.remove(0);
    // 禁止目标或父目录为链接、重复条目或可由非 root 改写的目录。
    let mut component = String::new();
    let parts: Vec<_> = path.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        if !component.is_empty() { component.push('/'); }
        component.push_str(part);
        let entries: Vec<_> = rd.entries.iter().filter(|e| e.name == component).collect();
        anyhow::ensure!(entries.len() == 1, "--adbd requires a unique parent directory /{component}");
        let entry = entries[0];
        anyhow::ensure!(entry.is_dir() && entry.uid == 0 && entry.mode & 0o022 == 0 && entry.mode & 0o100 != 0,
            "--adbd parent /{component} must be a root-owned, non-writable, traversable directory");
    }
    let entries: Vec<_> = rd.entries.iter().filter(|e| e.name == path).collect();
    anyhow::ensure!(entries.len() == 1, "--adbd requires a unique existing /{path}");
    anyhow::ensure!(entries[0].mode & 0o170000 == 0o100000 && entries[0].nlink == 1,
        "--adbd target /{path} must be a regular file without hard links");
    rd.replace(&path, binary)?;
    let entry = rd.entries.iter_mut().find(|e| e.name == path).unwrap();
    entry.mode = 0o100755;
    entry.uid = 0;
    entry.gid = 0;
    entry.rdev = (0, 0);
    Ok(path)
}

fn validate_static_adbd(data: &[u8]) -> Result<()> {
    let invalid = || anyhow::anyhow!("--adbd ELF is truncated or overflows");
    let u16_at = |at: usize| -> Result<u16> {
        Ok(u16::from_le_bytes(data.get(at..at + 2).ok_or_else(invalid)?.try_into()?))
    };
    let u32_at = |at: usize| -> Result<u32> {
        Ok(u32::from_le_bytes(data.get(at..at + 4).ok_or_else(invalid)?.try_into()?))
    };
    let u64_at = |at: usize| -> Result<u64> {
        Ok(u64::from_le_bytes(data.get(at..at + 8).ok_or_else(invalid)?.try_into()?))
    };
    anyhow::ensure!(data.len() >= 64 && &data[..4] == b"\x7fELF", "--adbd requires an ELF64 executable");
    anyhow::ensure!(data[4] == ELFCLASS64 && data[5] == ELFDATA2LSB && data[6] == 1 && u32_at(20)? == 1,
        "--adbd requires a little-endian ELF64 version 1");
    anyhow::ensure!(matches!(u16_at(16)?, ET_EXEC | ET_DYN) && u16_at(18)? == EM_AARCH64 && u16_at(52)? == 64,
        "--adbd requires an AArch64 ET_EXEC or static PIE executable");
    let phoff = usize::try_from(u64_at(32)?)?;
    let size = usize::from(u16_at(54)?);
    let count = usize::from(u16_at(56)?);
    anyhow::ensure!(phoff >= 64 && size == 56 && count > 0 && count < 0xffff, "--adbd invalid program-header table");
    let end = phoff.checked_add(size.checked_mul(count).ok_or_else(invalid)?).ok_or_else(invalid)?;
    anyhow::ensure!(end <= data.len(), "--adbd program headers are outside the file");
    let entry = u64_at(24)?;
    let mut executable_entry = false;
    for at in (phoff..end).step_by(size) {
        let kind = u32_at(at)?;
        anyhow::ensure!(kind != PT_INTERP, "--adbd has PT_INTERP; a static executable is required");
        let offset = usize::try_from(u64_at(at + 8)?)?;
        let filesz = usize::try_from(u64_at(at + 32)?)?;
        let file_end = offset.checked_add(filesz).ok_or_else(invalid)?;
        anyhow::ensure!(file_end <= data.len(), "--adbd segment is outside the file");
        if kind == 1 {
            let start = u64_at(at + 16)?;
            let limit = start.checked_add(filesz as u64).ok_or_else(invalid)?;
            anyhow::ensure!(u64_at(at + 40)? >= filesz as u64, "--adbd PT_LOAD memory size is too small");
            executable_entry |= u32_at(at + 4)? & 1 != 0 && entry >= start && entry < limit;
        }
        if kind == 2 {
            anyhow::ensure!(filesz >= 16 && filesz % 16 == 0, "--adbd invalid PT_DYNAMIC size");
            let mut terminated = false;
            for item in (offset..file_end).step_by(16) {
                let tag = u64_at(item)?;
                if tag == 0 { terminated = true; break; }
                anyhow::ensure!(!matches!(tag, 1 | 0x7fff_fffd | 0x7fff_ffff),
                    "--adbd has DT_NEEDED/DT_AUXILIARY/DT_FILTER shared-library dependencies");
            }
            anyhow::ensure!(terminated, "--adbd unterminated PT_DYNAMIC");
        }
    }
    anyhow::ensure!(entry != 0 && executable_entry, "--adbd entry point is not in a file-backed executable PT_LOAD");
    Ok(())
}

/// Replace the service binary where stock init already starts it. Prefer
/// the established system/bin layout when both paths exist.
pub fn install(rd: &mut Ramdisk, binary: Vec<u8>) -> Result<()> {
    let path = [paths::RECOVERY, "sbin/recovery"]
        .into_iter()
        .find(|path| rd.find(path).is_some())
        .context("stock ramdisk has neither system/bin/recovery nor sbin/recovery")?;
    rd.replace(path, binary)?;
    let (directory, recovery) = path
        .rsplit_once('/')
        .expect("the recovery binary is in a folder");
    // Scripts search /sbin as well as /system/bin. Keep the commands next
    // to the binary so their relative links work in either stock layout.
    for command in paths::COMMANDS {
        let (_, name) = command.rsplit_once('/').expect("a command is in a folder");
        let command = format!("{directory}/{name}");
        // Some stock releases carry unzip as an executable, others as a
        // link. Both deliberately become Skyward's streaming command.
        if rd
            .find(&command)
            .is_some_and(|e| e.mode & 0o170000 == 0o100000)
        {
            rd.drop_at(&command);
        }
        rd.link(&command, recovery)?;
    }
    Ok(())
}

/// Stock user-build adbd may enforce RSA authentication even when
/// ro.adb.secure=0. Its ramdisk key file works before /data is decrypted.
pub fn trust_adb_key(rd: &mut Ramdisk, key: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(key).context("ADB public key is not UTF-8")?;
    let text = text.trim();
    anyhow::ensure!(!text.contains("PRIVATE KEY"), "supply adbkey.pub, never the private adbkey");
    anyhow::ensure!(text.lines().count() == 1, "expected one adbkey.pub line");
    let encoded = text.split_whitespace().next().unwrap_or_default();
    // Android's 2048-bit RSA public-key structure is 524 bytes, encoded
    // as 700 base64 characters. Reject PEM, OpenSSH and private keys.
    anyhow::ensure!(encoded.len() == 700 && encoded.starts_with("QAAAA") &&
        encoded.ends_with('=') && encoded.bytes().take(699).all(|b|
            b.is_ascii_alphanumeric() || b == b'+' || b == b'/'),
        "expected an Android adbkey.pub (2048-bit RSA public key)");
    let mut keys = rd.find("adb_keys").map(|e| e.data.clone()).unwrap_or_default();
    if !keys.is_empty() && !keys.ends_with(b"\n") { keys.push(b'\n'); }
    keys.extend_from_slice(text.as_bytes());
    keys.push(b'\n');
    if rd.find("adb_keys").is_some() {
        rd.replace("adb_keys", keys)?;
    } else {
        rd.add("adb_keys", 0o644, keys)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock(path: &str) -> Ramdisk {
        let mut rd = Ramdisk {
            entries: Vec::new(),
        };
        rd.add(path, 0o750, b"stock recovery".to_vec()).unwrap();
        rd
    }

    fn assert_commands(rd: &Ramdisk, directory: &str) {
        for command in paths::COMMANDS {
            let name = command.rsplit_once('/').unwrap().1;
            let entry = rd.find(&format!("{directory}/{name}")).unwrap();
            assert_eq!(entry.mode & 0o170000, 0o120000);
            assert_eq!(entry.data, b"recovery");
        }
    }

    #[test]
    fn modern_layout_keeps_its_paths_and_binary_metadata() {
        let mut rd = stock(paths::RECOVERY);
        let entry = rd
            .entries
            .iter_mut()
            .find(|e| e.name == paths::RECOVERY)
            .unwrap();
        entry.uid = 1000;
        entry.gid = 2000;
        install(&mut rd, b"skyward".to_vec()).unwrap();
        let entry = rd.find(paths::RECOVERY).unwrap();
        assert_eq!(entry.data, b"skyward");
        assert_eq!((entry.mode, entry.uid, entry.gid), (0o100750, 1000, 2000));
        assert_commands(&rd, "system/bin");
        assert!(rd.find("sbin").is_none());
    }

    #[test]
    fn legacy_layout_uses_sbin_without_creating_system_bin() {
        let mut rd = stock("sbin/recovery");
        install(&mut rd, b"skyward".to_vec()).unwrap();
        assert_eq!(rd.find("sbin/recovery").unwrap().data, b"skyward");
        assert_commands(&rd, "sbin");
        assert!(rd.find("system").is_none());
    }

    #[test]
    fn modern_layout_takes_precedence_when_both_exist() {
        let mut rd = stock(paths::RECOVERY);
        rd.add("sbin/recovery", 0o755, b"legacy recovery".to_vec())
            .unwrap();
        install(&mut rd, b"skyward".to_vec()).unwrap();
        assert_eq!(rd.find(paths::RECOVERY).unwrap().data, b"skyward");
        assert_eq!(rd.find("sbin/recovery").unwrap().data, b"legacy recovery");
        assert_commands(&rd, "system/bin");
        assert!(rd.find("sbin/unzip").is_none());
    }

    #[test]
    fn executable_and_symlink_commands_are_replaced_in_both_layouts() {
        for directory in ["system/bin", "sbin"] {
            let mut rd = stock(&format!("{directory}/recovery"));
            rd.add(
                &format!("{directory}/unzip"),
                0o755,
                b"stock unzip".to_vec(),
            )
            .unwrap();
            rd.link(&format!("{directory}/zstd"), "toybox").unwrap();
            install(&mut rd, b"skyward".to_vec()).unwrap();
            assert_commands(&rd, directory);
        }
    }

    #[test]
    fn trusted_adb_key_preserves_stock_keys_and_adds_a_newline() {
        let key = format!("QAAAAA{}= computer", "A".repeat(693));
        let mut rd = stock("sbin/recovery");
        trust_adb_key(&mut rd, key.as_bytes()).unwrap();
        let expected = format!("{key}\n");
        assert_eq!(rd.find("adb_keys").unwrap().data, expected.as_bytes());
        rd.replace("adb_keys", b"stock-key".to_vec()).unwrap();
        trust_adb_key(&mut rd, key.as_bytes()).unwrap();
        assert_eq!(rd.find("adb_keys").unwrap().data, format!("stock-key\n{key}\n").as_bytes());
    }

    #[test]
    fn private_or_non_android_keys_are_rejected_without_changing_ramdisk() {
        let mut rd = stock("sbin/recovery");
        for key in ["-----BEGIN PRIVATE KEY-----", "ssh-rsa AAAA", "", "a\nb"] {
            assert!(trust_adb_key(&mut rd, key.as_bytes()).is_err());
            assert!(rd.find("adb_keys").is_none());
        }
    }

    #[test]
    fn missing_recovery_reports_both_supported_paths() {
        let mut rd = Ramdisk {
            entries: Vec::new(),
        };
        let error = install(&mut rd, Vec::new()).unwrap_err().to_string();
        assert!(error.contains("system/bin/recovery") && error.contains("sbin/recovery"));
        assert!(rd.entries.is_empty());
    }

    #[test]
    fn recovery_link_is_not_followed_or_overwritten() {
        let mut rd = Ramdisk {
            entries: Vec::new(),
        };
        rd.link(paths::RECOVERY, "elsewhere").unwrap();
        assert!(install(&mut rd, b"skyward".to_vec()).is_err());
        assert_eq!(rd.find(paths::RECOVERY).unwrap().data, b"elsewhere");
    }

    fn elf(kind: u16, machine: u16, interpreter: bool) -> Vec<u8> {
        let phoff = 64usize;
        let phentsize = 56usize;
        let phnum = if interpreter { 1 } else { 0 };
        let mut data = vec![0; phoff + phentsize * phnum];
        data[..4].copy_from_slice(b"\x7fELF");
        data[4] = ELFCLASS64;
        data[5] = ELFDATA2LSB;
        data[16..18].copy_from_slice(&kind.to_le_bytes());
        data[18..20].copy_from_slice(&machine.to_le_bytes());
        data[32..40].copy_from_slice(&(phoff as u64).to_le_bytes());
        data[52..54].copy_from_slice(&(64u16).to_le_bytes());
        data[54..56].copy_from_slice(&(phentsize as u16).to_le_bytes());
        data[56..58].copy_from_slice(&(phnum as u16).to_le_bytes());
        if interpreter {
            data[phoff..phoff + 4].copy_from_slice(&PT_INTERP.to_le_bytes());
        }
        data
    }

    fn static_adbd() -> Vec<u8> {
        let mut data = elf(ET_EXEC, EM_AARCH64, true);
        data[6] = 1;
        data[20..24].copy_from_slice(&1u32.to_le_bytes());
        data[24..32].copy_from_slice(&0x400000u64.to_le_bytes());
        data[64..68].copy_from_slice(&1u32.to_le_bytes());
        data[68..72].copy_from_slice(&5u32.to_le_bytes());
        data[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
        let len = data.len() as u64;
        data[96..104].copy_from_slice(&len.to_le_bytes());
        data[104..112].copy_from_slice(&len.to_le_bytes());
        data
    }

    fn stock_adbd(path: &str) -> Ramdisk {
        let mut rd = stock("sbin/recovery");
        rd.add(path, 0o750, b"stock adbd".to_vec()).unwrap();
        // Wayne 原厂 /sbin 为 0750，root 服务可遍历，无需放宽目录权限。
        rd.entries.iter_mut().find(|e| e.name == "sbin").unwrap().mode = 0o040750;
        rd.add("init.rc", 0o644, format!("service adbd /{path} --device_banner=recovery\n    disabled\n    seclabel u:r:adbd:s0\n").into_bytes()).unwrap();
        rd
    }

    #[test]
    fn adbd_replaces_only_the_declared_service_and_sets_safe_metadata() {
        for path in ["sbin/adbd", "system/bin/adbd"] {
            let mut rd = stock_adbd(path);
            let init = rd.find("init.rc").unwrap().data.clone();
            let other = if path == "sbin/adbd" { "system/bin/adbd" } else { "sbin/adbd" };
            rd.add(other, 0o755, b"unused adbd".to_vec()).unwrap();
            let target = rd.entries.iter_mut().find(|e| e.name == path).unwrap();
            target.mode = 0o106777;
            target.uid = 2000;
            target.gid = 2000;
            let data = static_adbd();
            assert_eq!(install_adbd(&mut rd, data.clone()).unwrap(), path);
            let entry = rd.find(path).unwrap();
            assert_eq!((entry.mode, entry.uid, entry.gid), (0o100755, 0, 0));
            assert_eq!(entry.data, data);
            assert_eq!(rd.find("init.rc").unwrap().data, init);
            assert_eq!(rd.find(other).unwrap().data, b"unused adbd");
            assert!(rd.find("adb_keys").is_none());
        }
    }

    #[test]
    fn adbd_rejects_ambiguous_missing_or_unsafe_service_targets() {
        for case in 0..8 {
            let mut rd = stock_adbd("sbin/adbd");
            match case {
                0 => { rd.drop_at("init.rc"); }
                1 => { rd.add("extra.rc", 0o644, b"service adbd /system/bin/adbd\n".to_vec()).unwrap(); }
                2 => { rd.replace("init.rc", b"service adbd /vendor/bin/adbd\n".to_vec()).unwrap(); }
                3 => { rd.drop_at("sbin/adbd"); rd.link("sbin/adbd", "/other").unwrap(); }
                4 => { rd.entries.iter_mut().find(|e| e.name == "sbin").unwrap().mode = 0o040777; }
                5 => { rd.entries.iter_mut().find(|e| e.name == "sbin").unwrap().mode = 0o120755; }
                6 => { let duplicate = rd.find("sbin/adbd").unwrap().clone(); rd.entries.push(duplicate); }
                7 => { rd.entries.iter_mut().find(|e| e.name == "sbin/adbd").unwrap().nlink = 2; }
                _ => unreachable!(),
            }
            let before = rd.archive().unwrap();
            assert!(install_adbd(&mut rd, static_adbd()).is_err(), "case {case}");
            assert_eq!(rd.archive().unwrap(), before);
        }
    }

    #[test]
    fn adbd_rejects_invalid_elf_headers_segments_and_entry_points() {
        for (at, bytes) in [
            (4, vec![1]), (5, vec![2]), (6, vec![0]),
            (16, 1u16.to_le_bytes().to_vec()), (18, 62u16.to_le_bytes().to_vec()),
            (24, 0u64.to_le_bytes().to_vec()), (32, u64::MAX.to_le_bytes().to_vec()),
            (54, 8u16.to_le_bytes().to_vec()), (56, 0u16.to_le_bytes().to_vec()),
            (64, PT_INTERP.to_le_bytes().to_vec()), (68, 4u32.to_le_bytes().to_vec()),
            (72, u64::MAX.to_le_bytes().to_vec()), (96, u64::MAX.to_le_bytes().to_vec()),
        ] {
            let mut data = static_adbd();
            data[at..at + bytes.len()].copy_from_slice(&bytes);
            assert!(validate_static_adbd(&data).is_err(), "offset {at}");
        }
        assert!(validate_static_adbd(b"not ELF").is_err());
        let mut pie = static_adbd();
        pie[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        validate_static_adbd(&pie).unwrap();
    }

    #[test]
    fn adbd_rejects_shared_dependencies_even_without_an_interpreter() {
        for tag in [0u64, 1, 0x7fff_fffd, 0x7fff_ffff, 5] {
            let mut data = static_adbd();
            data.resize(192, 0);
            data[56..58].copy_from_slice(&2u16.to_le_bytes());
            data[120..124].copy_from_slice(&2u32.to_le_bytes());
            data[128..136].copy_from_slice(&176u64.to_le_bytes());
            data[152..160].copy_from_slice(&16u64.to_le_bytes());
            data[176..184].copy_from_slice(&tag.to_le_bytes());
            assert_eq!(validate_static_adbd(&data).is_ok(), tag == 0, "dynamic tag {tag}");
        }
    }

    #[test]
    fn shell_keeps_an_existing_system_shell() {
        let mut rd = stock(paths::RECOVERY);
        rd.add(SHELL, 0o755, b"stock shell".to_vec()).unwrap();
        install_shell(&mut rd, Some(elf(ET_EXEC, EM_AARCH64, false))).unwrap();
        assert_eq!(rd.find(SHELL).unwrap().data, b"stock shell");
    }

    #[test]
    fn shell_aliases_each_executable_stock_candidate() {
        for candidate in STOCK_SHELLS {
            let mut rd = stock(paths::RECOVERY);
            rd.add(candidate, 0o755, candidate.as_bytes().to_vec()).unwrap();
            install_shell(&mut rd, None).unwrap();
            let shell = rd.find(SHELL).unwrap();
            assert_eq!(shell.mode & 0o170000, 0o120000);
            assert_eq!(shell.data, format!("/{candidate}").as_bytes());
        }
    }

    #[test]
    fn missing_optional_shell_keeps_existing_packing_workflow() {
        let mut rd = stock(paths::RECOVERY);
        install_shell(&mut rd, None).unwrap();
        assert!(rd.find(SHELL).is_none());
    }

    #[test]
    fn shell_accepts_static_aarch64_exec_and_dyn_elves() {
        for kind in [ET_EXEC, ET_DYN] {
            let mut rd = stock(paths::RECOVERY);
            let data = elf(kind, EM_AARCH64, false);
            install_shell(&mut rd, Some(data.clone())).unwrap();
            assert_eq!(rd.find(SHELL).unwrap().data, data);
        }
    }

    #[test]
    fn shell_rejects_interpreted_or_non_aarch64_elves() {
        for data in [
            elf(ET_EXEC, EM_AARCH64, true),
            elf(ET_EXEC, 62, false),
            elf(1, EM_AARCH64, false),
            b"not an elf".to_vec(),
        ] {
            let mut rd = stock(paths::RECOVERY);
            assert!(install_shell(&mut rd, Some(data)).is_err());
            assert!(rd.find(SHELL).is_none());
        }
    }
}
