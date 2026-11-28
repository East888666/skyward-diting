//! Run pre-APEX executables with their own linker using RAM-only copies.
use anyhow::{ensure, Context, Result};
use std::{fs, path::{Path, PathBuf}, hash::{Hash, Hasher}, os::unix::fs::PermissionsExt};

fn patch(elf: &mut [u8], interpreter: &str) -> Result<()> {
    ensure!(elf.len() >= 64 && &elf[..7] == b"\x7fELF\x02\x01\x01", "interpreter requires a little-endian ELF64 executable");
    let offset = u64::from_le_bytes(elf[32..40].try_into()?) as usize;
    let stride = u16::from_le_bytes(elf[54..56].try_into()?) as usize;
    let count = u16::from_le_bytes(elf[56..58].try_into()?) as usize;
    ensure!(stride >= 56 && count <= 1024, "invalid ELF program headers");
    let mut target = None;
    for index in 0..count {
        let start = offset.checked_add(index.checked_mul(stride).context("ELF header overflow")?).context("ELF header overflow")?;
        let header = elf.get(start..start.checked_add(56).context("ELF header overflow")?).context("truncated ELF header")?;
        if u32::from_le_bytes(header[..4].try_into()?) == 3 {
            ensure!(target.is_none(), "multiple ELF interpreters");
            let at = usize::try_from(u64::from_le_bytes(header[8..16].try_into()?))?;
            let size = usize::try_from(u64::from_le_bytes(header[32..40].try_into()?))?;
            target = Some((at, size));
        }
    }
    let (at, size) = target.context("ELF has no interpreter")?;
    ensure!(!interpreter.as_bytes().contains(&0) && interpreter.len() < size && size <= 4096, "replacement interpreter does not fit");
    let bytes = elf.get_mut(at..at.checked_add(size).context("ELF interpreter overflow")?).context("truncated ELF interpreter")?;
    ensure!(bytes.contains(&0), "unterminated ELF interpreter");
    bytes.fill(0);
    bytes[..interpreter.len()].copy_from_slice(interpreter.as_bytes());
    Ok(())
}

pub fn stage(program: &str, loader: &str) -> Result<PathBuf> {
    ensure!(Path::new(loader).is_absolute() && Path::new(loader).is_file(), "installed interpreter is missing: {loader}");
    let mut elf = fs::read(program).with_context(|| format!("read {program} for RAM interpreter staging"))?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    program.hash(&mut hash); loader.hash(&mut hash); elf.hash(&mut hash);
    let identity = hash.finish();
    // Short enough for the old /system/bin/linker64 PT_INTERP slot.
    let alias = format!("/sld/{:08x}", identity as u32);
    fs::create_dir_all("/sld")?;
    fs::set_permissions("/sld", fs::Permissions::from_mode(0o755))?;
    match fs::read_link(&alias) {
        Ok(existing) => ensure!(existing == Path::new(loader), "interpreter alias collision"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::os::unix::fs::symlink(loader, &alias)?,
        Err(e) => return Err(e.into()),
    }
    patch(&mut elf, &alias)?;
    // Keep executables outside run/, which may be private under init's
    // umask because it also holds diagnostic logs and runtime state.
    let dir = Path::new("/skyward/interpreters");
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
    let name = Path::new(program).file_name().context("service has no file name")?.to_string_lossy();
    let to = dir.join(format!("{identity:016x}-{name}"));
    if !to.exists() {
        let part = to.with_extension("part");
        fs::write(&part, &elf)?;
        fs::set_permissions(&part, fs::Permissions::from_mode(0o755))?;
        fs::rename(part, &to)?;
    }
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn patches_only_the_bounded_interpreter_slot() {
        let mut elf = vec![0; 160]; elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes()); elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&3u32.to_le_bytes()); elf[72..80].copy_from_slice(&128u64.to_le_bytes());
        elf[96..104].copy_from_slice(&24u64.to_le_bytes());
        elf[128..149].copy_from_slice(b"/system/bin/linker64\0");
        let before = elf.clone(); patch(&mut elf, "/sld/12345678").unwrap();
        assert_eq!(&elf[..128], &before[..128]); assert_eq!(&elf[152..], &before[152..]);
        assert_eq!(&elf[128..128 + b"/sld/12345678\0".len()], b"/sld/12345678\0");
        assert!(patch(&mut elf, "/an/interpreter/path/that/does/not/fit").is_err());
        assert!(patch(&mut elf[..100], "/sld/x").is_err());
    }
}
