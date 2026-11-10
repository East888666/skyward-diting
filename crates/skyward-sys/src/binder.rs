//! Binder devices that stock recoveries do not always create.

use anyhow::{Context, Result};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

const BINDERFS: &str = "/dev/binderfs";
const BINDERFS_MAGIC: u64 = 0x6c6f6f70;

#[repr(C)]
struct Device {
    name: [u8; 256],
    major: u32,
    minor: u32,
}

const BINDER_CTL_ADD: u32 =
    (3 << 30) | ((size_of::<Device>() as u32) << 16) | ((b'b' as u32) << 8) | 1;

/// Reuse legacy devices, or mount binderfs and allocate missing devices.
/// Never replace a working device: all services must share its context.
pub fn prepare() -> Result<()> {
    let names = ["binder", "hwbinder", "vndbinder"];
    if names.iter().any(|n| !Path::new("/dev").join(n).exists()) {
        fs::create_dir_all(BINDERFS).context("make binderfs mount point")?;
        let mut info: libc::statfs = unsafe { std::mem::zeroed() };
        let at = c"/dev/binderfs";
        if unsafe { libc::statfs(at.as_ptr(), &mut info) } != 0 {
            return Err(std::io::Error::last_os_error()).context("inspect binderfs mount point");
        }
        if info.f_type != BINDERFS_MAGIC
            && unsafe {
                libc::mount(
                    c"binder".as_ptr(),
                    at.as_ptr(),
                    c"binder".as_ptr(),
                    0,
                    std::ptr::null(),
                )
            } != 0
        {
            return Err(std::io::Error::last_os_error()).context("mount binderfs");
        }
        fs::set_permissions(BINDERFS, fs::Permissions::from_mode(0o755))
            .context("open binderfs to services")?;
        let control = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(Path::new(BINDERFS).join("binder-control"))
            .context("open binder-control")?;
        for name in names {
            let alias = Path::new("/dev").join(name);
            if alias.exists() {
                continue;
            }
            let node = Path::new(BINDERFS).join(name);
            if !node.exists() {
                let mut device = Device {
                    name: [0; 256],
                    major: 0,
                    minor: 0,
                };
                device.name[..name.len()].copy_from_slice(name.as_bytes());
                if unsafe {
                    libc::ioctl(
                        control.as_raw_fd(),
                        BINDER_CTL_ADD as _,
                        &mut device,
                    )
                } != 0
                {
                    return Err(std::io::Error::last_os_error())
                        .with_context(|| format!("allocate {name} in binderfs"));
                }
            }
            if alias.symlink_metadata().is_ok() {
                fs::remove_file(&alias)
                    .with_context(|| format!("remove broken {}", alias.display()))?;
            }
            symlink(&node, &alias)
                .with_context(|| format!("link {} to {}", alias.display(), node.display()))?;
        }
    }
    for name in names {
        let node = Path::new("/dev").join(name);
        fs::set_permissions(&node, fs::Permissions::from_mode(0o666))
            .with_context(|| format!("open {} to services", node.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_matches_linux_binderfs_uapi() {
        assert_eq!(size_of::<Device>(), 264);
        assert_eq!(BINDER_CTL_ADD, 0xc1086201);
    }
}
