//! Making a block device out of part of a file, which is how the bundles a
//! phone keeps its shared libraries in are opened.

#[cfg(target_os = "android")]
pub use android::*;

#[cfg(target_os = "android")]
mod android {
    use anyhow::{Context, Result};
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};

    const CONTROL: &str = "/dev/loop-control";

    const GET_FREE: u32 = 0x4C82;
    const CONFIGURE: u32 = 0x4C0A;
    const SET_FD: u32 = 0x4C00;
    const CLR_FD: u32 = 0x4C01;
    const SET_STATUS64: u32 = 0x4C04;

    /// The device only ever reads. It stays pointed at the file until the
    /// recovery ends, which is as long as anything mounted from it lasts.
    const READ_ONLY: u32 = 1;

    /// Every field is a plain number or a run of bytes, so the request is
    /// made by zeroing it and filling in what matters.
    #[repr(C)]
    struct Info {
        device: u64,
        inode: u64,
        rdevice: u64,
        offset: u64,
        size_limit: u64,
        number: u32,
        encrypt_type: u32,
        encrypt_key_size: u32,
        flags: u32,
        file_name: [u8; 64],
        crypt_name: [u8; 64],
        encrypt_key: [u8; 32],
        init: [u64; 2],
    }

    #[repr(C)]
    struct Config {
        fd: u32,
        block_size: u32,
        info: Info,
        reserved: [u64; 8],
    }

    /// Open `file` from `offset` for `length` bytes as a block device, and
    /// give its path. The association stays until detached or rebooted.
    pub fn attach(file: &Path, offset: u64, length: u64) -> Result<PathBuf> {
        let backing = File::open(file).with_context(|| format!("open {}", file.display()))?;
        let control = File::open(CONTROL).with_context(|| format!("open {CONTROL}"))?;
        let number = unsafe { libc::ioctl(control.as_raw_fd(), GET_FREE as libc::Ioctl) };
        if number < 0 {
            return Err(std::io::Error::last_os_error()).context("ask for a free loop device");
        }
        let path = free_device(number as u32)?;
        let device = File::options()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        let mut config: Config = unsafe { std::mem::zeroed() };
        config.fd = backing.as_raw_fd() as u32;
        config.info.offset = offset;
        config.info.size_limit = length;
        config.info.flags = READ_ONLY;
        if unsafe { libc::ioctl(device.as_raw_fd(), CONFIGURE as libc::Ioctl, &mut config) } < 0 {
            let error = std::io::Error::last_os_error();
            // LOOP_CONFIGURE arrived after the 4.9 kernels used by older
            // recoveries. They report EINVAL or ENOTTY for this command.
            if !matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOTTY | libc::ENOSYS)) {
                return Err(error).with_context(|| format!("point {} at {}", path.display(), file.display()));
            }
            if unsafe { libc::ioctl(device.as_raw_fd(), SET_FD as libc::Ioctl, backing.as_raw_fd()) } < 0 {
                return Err(std::io::Error::last_os_error()).context("attach legacy loop backing file");
            }
            if unsafe { libc::ioctl(device.as_raw_fd(), SET_STATUS64 as libc::Ioctl, &config.info) } < 0 {
                let error = std::io::Error::last_os_error();
                unsafe { libc::ioctl(device.as_raw_fd(), CLR_FD as libc::Ioctl, 0) };
                return Err(error).context("set legacy loop offset, size and read-only flag");
            }
        }
        Ok(path)
    }

    /// Release an association after every mount of this loop has gone.
    /// No autoclear/lazy detach: a live user must still block installation.
    pub fn detach(path: &Path) -> Result<()> {
        let device = File::options().read(true).write(true).open(path)
            .with_context(|| format!("open {}", path.display()))?;
        if unsafe { libc::ioctl(device.as_raw_fd(), CLR_FD as libc::Ioctl, 0) } < 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| format!("release loop {}", path.display()));
        }
        Ok(())
    }

    /// Where the kernel put the loop device it just handed out: Android
    /// keeps its block devices in their own directory.
    fn free_device(number: u32) -> Result<PathBuf> {
        for dir in ["/dev/block", "/dev"] {
            let path = Path::new(dir).join(format!("loop{number}"));
            if path.exists() {
                return Ok(path);
            }
        }
        anyhow::bail!("the kernel handed out loop device {number}, which is in neither /dev/block nor /dev")
    }
}
