//! Read-only Android cryptfs footer parsing and KDF 5 derivation.
//! Hardware activation is kept separate from parsing or password derivation.
use anyhow::{ensure, Context, Result};
use scrypt::{scrypt, Params};

pub const FOOTER_RESERVED: u64 = 16384;
pub const FOOTER_BYTES: usize = 2352;

#[cfg(target_os = "android")]
mod ice {
    use super::*;
    use std::{fs::{self, File, OpenOptions}, os::{fd::AsRawFd, unix::fs::{FileExt, OpenOptionsExt}}, path::Path};
    const STATE: &str = "/skyward/run/cryptfs-ice";
    const QSEE_SET_ICE: libc::c_int = 0xc004972bu32 as libc::c_int;
    const QSEE_LOAD_KEY: libc::c_int = 0xc0249711u32 as libc::c_int;
    const BLKFLSBUF: libc::c_int = 0x1261;

    pub fn read_footer(device: &Path) -> Result<Option<Footer>> {
        let size = skyward_block::size_bytes(device)?;
        if size < FOOTER_RESERVED { return Ok(None); }
        let disk = File::open(device)?;
        let mut raw = vec![0; FOOTER_BYTES];
        disk.read_exact_at(&mut raw, size - FOOTER_RESERVED)?;
        let result = Footer::parse(&raw, size);
        erase(&mut raw);
        result
    }

    pub fn active() -> bool { Path::new(STATE).exists() }

    pub struct Operation { _file: File }
    pub fn lock() -> Result<Operation> {
        fs::create_dir_all("/skyward/run")?;
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open("/skyward/run/cryptfs-ice.lock")?;
        ensure!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0, "another cryptfs operation is running");
        Ok(Operation { _file: file })
    }

    fn flag(qsee: &File, mut value: i32) -> Result<()> {
        ensure!(unsafe { libc::ioctl(qsee.as_raw_fd(), QSEE_SET_ICE, &mut value) } == 0,
            "set cryptfs ICE mode: {}", std::io::Error::last_os_error());
        Ok(())
    }

    // The kernel mode outlives this process. Track it in RAM across UI and
    // CLI instances rather than pretending a process-local bool owns it.
    pub struct Activation<'a> { committed: bool, operation: &'a Operation }
    impl Activation<'_> { pub fn commit(mut self) { self.committed = true; } }
    impl Drop for Activation<'_> {
        fn drop(&mut self) {
            if !self.committed && let Err(error) = deactivate(self.operation) {
                eprintln!("cryptfs cleanup needs attention: {error:#}");
            }
        }
    }

    pub fn activate<'a>(device: &Path, derived: &Derived, operation: &'a Operation) -> Result<Activation<'a>> {
        ensure!(!active(), "cryptfs ICE is already active");
        ensure!(!skyward_block::mount::is_mount_point(Path::new(crate::data::MOUNT))?, "close data before changing ICE mode");
        let qsee = OpenOptions::new().read(true).write(true).open("/dev/qseecom")?;
        #[repr(C)] struct LoadKey { hash: [u8; 32], usage: u32 }
        let mut key = LoadKey { hash: derived.hardware_password(), usage: 4 };
        let result = unsafe { libc::ioctl(qsee.as_raw_fd(), QSEE_LOAD_KEY, &mut key) };
        erase(&mut key.hash);
        ensure!(result == 0, "load existing cryptfs SDCC key: {}", std::io::Error::last_os_error());
        let disk = OpenOptions::new().read(true).write(true).open(device)?;
        ensure!(unsafe { libc::ioctl(disk.as_raw_fd(), BLKFLSBUF) } == 0,
            "invalidate ciphertext cache before cryptfs activation: {}", std::io::Error::last_os_error());
        let device = fs::canonicalize(device)?;
        fs::write(STATE, device.as_os_str().as_encoded_bytes())?;
        let activation = Activation { committed: false, operation };
        // Both bits: never let writable ext4 submit plaintext writes under
        // a decrypt-only mode. The footer is excluded by the kernel too.
        flag(&qsee, 3)?;
        verify_ext4(&device)?;
        Ok(activation)
    }

    fn verify_ext4(device: &Path) -> Result<()> {
        let disk = OpenOptions::new().read(true).custom_flags(libc::O_DIRECT).open(device)?;
        let mut ptr = std::ptr::null_mut();
        ensure!(unsafe { libc::posix_memalign(&mut ptr, 4096, 4096) } == 0, "allocate cryptfs verification buffer");
        let read = unsafe { libc::pread(disk.as_raw_fd(), ptr, 4096, 0) };
        let valid = read == 4096 && unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(),4096) }[1080..1082] == [0x53,0xef];
        unsafe { libc::memset(ptr, 0, 4096); libc::free(ptr); }
        ensure!(valid, "cryptfs ICE did not decrypt a valid ext4 superblock");
        Ok(())
    }

    /// Called only after all data mounts and the bounded dm view are gone.
    /// A failure leaves the marker, so the next attempt cannot silently
    /// treat an active global ICE mode as plain data.
    pub fn deactivate(_operation: &Operation) -> Result<()> {
        if !active() { return Ok(()); }
        ensure!(!skyward_block::mount::is_mount_point(Path::new(crate::data::MOUNT))?
            && skyward_block::dm::status(crate::data::MAPPED)?.is_none(), "unmount and remove cryptfs data before resetting ICE");
        let text = fs::read_to_string(STATE)?;
        let path = Path::new(&text);
        ensure!(path.starts_with("/dev/block/") && path.components().all(|c| !matches!(c, std::path::Component::ParentDir)), "invalid cryptfs state");
        let disk = OpenOptions::new().read(true).write(true).open(path)?;
        ensure!(unsafe { libc::ioctl(disk.as_raw_fd(), BLKFLSBUF) } == 0,
            "flush decrypted cache before cryptfs reset: {}", std::io::Error::last_os_error());
        let qsee = OpenOptions::new().read(true).write(true).open("/dev/qseecom")?;
        flag(&qsee, 0)?;
        fs::remove_file(STATE)?;
        Ok(())
    }
}

#[cfg(target_os = "android")]
pub use ice::{Operation, active, activate, deactivate, lock, read_footer};

/// No Debug implementation: this contains a hardware key blob.
pub struct Footer {
    pub sectors: u64,
    pub default_password: bool,
    salt: [u8; 16],
    factors: [u8; 3],
    blob: Vec<u8>,
    verifier: [u8; 32],
}

impl Footer {
    pub fn parse(raw: &[u8], device_bytes: u64) -> Result<Option<Self>> {
        ensure!(raw.len() >= FOOTER_BYTES, "short cryptfs footer");
        let u32_at = |at| u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
        if u32_at(0) != 0xd0b5b1c4 { return Ok(None); }
        let major = u16::from_le_bytes(raw[4..6].try_into()?);
        let minor = u16::from_le_bytes(raw[6..8].try_into()?);
        ensure!(major == 1 && minor == 3, "unsupported cryptfs footer version {major}.{minor}");
        ensure!(u32_at(8) as usize == FOOTER_BYTES, "unsupported cryptfs footer size");
        ensure!(u32_at(12) & !0x1000 == 0, "cryptfs footer is incomplete or needs recovery; refusing to change it");
        ensure!(u32_at(16) == 16, "unsupported hardware cryptfs key size");
        let cipher = raw[36..100].split(|&b| b == 0).next().unwrap_or_default();
        ensure!(cipher == b"aes-xts", "unsupported hardware cryptfs cipher");
        ensure!(raw[188] == 5, "only cryptfs scrypt-Keymaster KDF 5 is supported");
        let sectors = u64::from_le_bytes(raw[24..32].try_into()?);
        let end = sectors.checked_mul(512).context("cryptfs sector length overflow")?;
        ensure!(sectors > 0 && device_bytes.checked_sub(FOOTER_RESERVED).is_some_and(|limit| end == limit),
            "cryptfs filesystem does not end at its footer");
        let blob_size = u32_at(2280) as usize;
        ensure!(blob_size > 0 && blob_size <= 2048, "invalid cryptfs keymaster blob length");
        let footer = Self {
            sectors, default_password: u32_at(20) == 1,
            salt: raw[152..168].try_into()?, factors: raw[189..192].try_into()?,
            blob: raw[232..232 + blob_size].to_vec(), verifier: raw[2284..2316].try_into()?,
        };
        footer.params()?;
        Ok(Some(footer))
    }
    fn params(&self) -> Result<Params> {
        let [n, r, p] = self.factors;
        // Bound both memory and CPU before using untrusted on-disk factors.
        ensure!((1..=18).contains(&n) && r <= 5 && p <= 4 && n as u32 + r as u32 <= 21
            && n as u32 + r as u32 + p as u32 <= 24, "cryptfs scrypt parameters exceed recovery limits");
        Params::new(n, 1u32 << r, 1u32 << p, 32).map_err(|_| anyhow::anyhow!("invalid cryptfs scrypt parameters"))
    }
    /// Only uncredentialed FDE is enabled at present. No password guessing.
    pub fn derive_default(&self, sign: impl FnOnce(&[u8], &[u8; 256]) -> Result<Vec<u8>>) -> Result<Derived> {
        ensure!(self.default_password, "credential-protected legacy FDE is not supported yet");
        let params = self.params()?;
        let mut first = Derived([0; 32]);
        scrypt(b"default_password", &self.salt, &params, &mut first.0).map_err(|_| anyhow::anyhow!("cryptfs scrypt output length"))?;
        let mut input = [0u8; 256];
        input[1..33].copy_from_slice(&first.0);
        let signature = sign(&self.blob, &input);
        erase(&mut input);
        let mut signature = signature?;
        ensure!(signature.len() == 256, "cryptfs requires a 256-byte RSA signature");
        let mut derived = Derived([0; 32]);
        let result = scrypt(&signature, &self.salt, &params, &mut derived.0);
        erase(&mut signature);
        result.map_err(|_| anyhow::anyhow!("cryptfs scrypt output length"))?;
        Ok(derived)
    }
    pub fn verifier_present(&self) -> bool { self.verifier.iter().any(|&b| b != 0) }
    pub fn verify(&self, derived: &Derived) -> Result<bool> {
        ensure!(self.verifier_present(), "cryptfs footer has no intermediate-key verifier");
        let mut check = Derived([0; 32]);
        scrypt(&derived.0[..16], &self.salt, &self.params()?, &mut check.0).map_err(|_| anyhow::anyhow!("cryptfs verifier output length"))?;
        Ok(check.0.iter().zip(self.verifier).fold(0u8, |diff, (&a, b)| diff | (a ^ b)) == 0)
    }
}

pub struct Derived([u8; 32]);
impl Derived {
    /// Qualcomm cryptfs_hw treats the derived binary password as a bounded
    /// C string; mirror that ABI, including its zero padding.
    pub fn hardware_password(&self) -> [u8; 32] {
        let length = self.0.iter().position(|&b| b == 0).unwrap_or(32);
        let mut result = [0; 32]; result[..length].copy_from_slice(&self.0[..length]); result
    }
}
impl Drop for Derived { fn drop(&mut self) { erase(&mut self.0); } }
fn erase(bytes: &mut [u8]) {
    for byte in bytes { unsafe { std::ptr::write_volatile(byte, 0); } }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid() -> Vec<u8> {
        let mut raw = vec![0; FOOTER_BYTES];
        for (at, n) in [(0, 0xd0b5b1c4u32), (8, 2352), (12, 0x1000), (16, 16), (20, 1), (2280, 64)] {
            raw[at..at+4].copy_from_slice(&n.to_le_bytes());
        }
        raw[4..6].copy_from_slice(&1u16.to_le_bytes()); raw[6..8].copy_from_slice(&3u16.to_le_bytes());
        raw[24..32].copy_from_slice(&32u64.to_le_bytes()); raw[36..43].copy_from_slice(b"aes-xts");
        raw[188..192].copy_from_slice(&[5, 4, 0, 0]); raw
    }
    #[test] fn footer_bounds_and_untrusted_costs() {
        let bytes = 32768;
        assert!(Footer::parse(&valid(), bytes).unwrap().is_some());
        let mut raw = valid(); raw[189] = 255; assert!(Footer::parse(&raw, bytes).is_err());
        let mut raw = valid(); raw[12] = 2; assert!(Footer::parse(&raw, bytes).is_err());
        let mut raw = valid(); raw[2280..2284].copy_from_slice(&2049u32.to_le_bytes()); assert!(Footer::parse(&raw, bytes).is_err());
        assert!(Footer::parse(&valid()[..100], bytes).is_err());
        assert!(Footer::parse(&valid(), bytes+512).is_err());
        let mut raw = valid(); raw[..4].fill(0); assert!(Footer::parse(&raw, bytes).unwrap().is_none());
    }
    #[test] fn kdf5_padding_and_verification() {
        let mut raw = valid(); raw[2284] = 1;
        let footer = Footer::parse(&raw, 32768).unwrap().unwrap();
        let derived = footer.derive_default(|blob, input| {
            assert_eq!(blob.len(), 64); assert_eq!(input[0], 0); assert!(input[33..].iter().all(|&b| b == 0));
            let mut first = [0; 32]; scrypt(b"default_password", &[0;16], &Params::new(4,1,1,32).unwrap(), &mut first).unwrap();
            assert_eq!(&input[1..33], &first); Ok(vec![7;256])
        }).unwrap();
        assert_eq!(hex::encode(derived.0), "743f4babb9b6d2d80a61eb2d714e1268aa86299cb7cb5e8f5d8ade2899a60624"); // independent hashlib.scrypt vector
        assert!(!footer.verify(&derived).unwrap());
        let mut expected = [0;32]; scrypt(&derived.0[..16], &[0;16], &Params::new(4,1,1,32).unwrap(), &mut expected).unwrap();
        raw[2284..2316].copy_from_slice(&expected);
        assert!(Footer::parse(&raw,32768).unwrap().unwrap().verify(&derived).unwrap());
    }
    #[test] fn hardware_password_preserves_legacy_c_string_semantics() {
        let mut bytes = [4;32]; bytes[3] = 0;
        assert_eq!(&Derived(bytes).hardware_password()[..6], &[4,4,4,0,0,0]);
    }
}
