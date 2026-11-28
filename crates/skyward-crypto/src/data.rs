//! Opening the data partition. A phone encrypts the whole of it with a key
//! its storage hardware holds, and encrypts each user's files again with a
//! key only that user's credential leads to. This is the first of the two:
//! what it opens holds every file, with the names and contents of the ones
//! that are a user's own still encrypted. A phone whose own table has the
//! partition encrypted in neither way keeps it as the plain filesystem it
//! is, which opens with no key and no key service.

use crate::encryption::{self, key_dir, target, Encryption};
use crate::gatekeeper::Gatekeeper;
use crate::keymint::KeyMint;
use crate::keystore::{StoredKey, Unwrap};
use crate::token::Token;
use crate::user::Bound;
use crate::weaver::Weaver;
use anyhow::{Context, Result};
use skyward_binder::driver::Binder;
use skyward_block::fsck::Checked;
use skyward_block::fstab::Entry;
use skyward_block::{dm, mount};
use std::path::{Path, PathBuf};

/// Where the data partition is mounted, and the name its decrypted form is
/// given among the device-mapper's devices.
pub const MOUNT: &str = "/data";
pub const MAPPED: &str = "userdata";

/// Where the phone keeps the files its users share between apps, a folder
/// for each user, and where the recovery shows the owner's, the way the
/// phone shows them to apps.
pub const SHARED_STORAGE: &str = "/data/media";
pub const SDCARD: &str = "/sdcard";

/// The phone's owner, the first of its users.
const OWNER: u32 = 0;

/// Unwrap the key the phone keeps for its data partition and give the
/// storage hardware the form of it that lasts until the phone is started
/// again.
pub fn storage_key(service: &KeyMint, entry: &Entry, encryption: &Encryption) -> Result<Vec<u8>> {
    hardware_key(service, &key_dir(entry)?, encryption.wrapped)
}

/// The key a directory holds, in the form the storage hardware takes for
/// this start of the phone. A key the hardware holds never appears as
/// itself: what comes back is wrapped again, for this start only.
fn hardware_key(service: &KeyMint, dir: &Path, wrapped: bool) -> Result<Vec<u8>> {
    let stored = StoredKey::load(dir).with_context(|| format!("read the key in {}", dir.display()))?;
    let key = stored.unwrap(Unwrap::Hardware(service)).with_context(|| format!("unwrap the key in {}", dir.display()))?;
    match wrapped {
        true => service.ephemeral_storage_key(&key).with_context(|| format!("make the key in {} usable now", dir.display())),
        false => Ok(key),
    }
}

/// What the phone's own table says about its data partition: every entry
/// it has for it, the one the system tries first first.
fn table() -> Result<Vec<Entry>> {
    let entries = skyward_block::fstab::read(MOUNT)?;
    anyhow::ensure!(!entries.is_empty(), "the phone's own table says nothing about {MOUNT}");
    Ok(entries)
}

/// Whether opening the data partition takes the phone's key service: it
/// does where the phone's table has the partition encrypted whole or its
/// files encrypted one by one. A partition the table has in neither way
/// opens on a phone whose key service is not running.
pub fn keyed() -> Result<bool> {
    let entries = table()?;
    Ok(encryption::whole(&entries)?.is_some() || encryption::files(&entries))
}

/// The key service a partition the phone encrypts is opened with.
fn key_service<'a, 'b>(keys: Option<&'a KeyMint<'b>>) -> Result<&'a KeyMint<'b>> {
    keys.context("the phone's table has its data partition encrypted, and the key service was not reached for it")
}

/// The data partition as [`open`] leaves it.
pub struct Opened {
    /// The device the partition is read and written through: decrypted
    /// where the phone encrypts the partition whole, and the partition
    /// itself where it does not.
    pub mapped: PathBuf,
    pub filesystem: String,
    /// How the check before mounting ended, where the phone's table asks
    /// for one.
    pub checked: Option<Checked>,
}

/// Open the phone's data partition: where the phone encrypts it whole,
/// unwrap the key it is encrypted with and give that to the storage
/// hardware; check the filesystem where the phone's table asks for it,
/// and mount it writable, as the phone mounts it. The names and contents
/// of each user's own files stay encrypted until that user's key is let in
/// as well. A partition that does not mount leaves no device behind.
fn open(keys: Option<&KeyMint>, entries: &[Entry]) -> Result<Opened> {
    let reached = reach(keys, entries)?;
    let mounted = (|| -> Result<(Option<Checked>, String)> {
        // A phone that names the partition once per filesystem it might
        // hold mounts it with the entry of the one it holds.
        let holds = skyward_block::probe::filesystem(&reached.device)?;
        let entry = entries.iter().find(|e| holds.as_ref() == Some(&e.filesystem)).unwrap_or(&entries[0]);
        let checked = match entry.extras.iter().any(|e| e == "check") {
            true => Some(skyward_block::fsck::check(&reached.device, &entry.filesystem)?),
            false => None,
        };
        std::fs::create_dir_all(MOUNT).with_context(|| format!("make {MOUNT}"))?;
        mount::mount(&reached.device, Path::new(MOUNT), &entry.filesystem, &entry.options)?;
        Ok((checked, entry.filesystem.clone()))
    })();
    match mounted {
        Ok((checked, filesystem)) => Ok(Opened { mapped: reached.device, filesystem, checked }),
        Err(e) => match reached.close() {
            Ok(()) => Err(e),
            Err(r) => Err(e.context(format!("removing {MAPPED} afterwards failed too: {r:#}"))),
        },
    }
}

/// The device the data partition's filesystem is read and written through,
/// with nothing mounted from it.
pub struct Reached {
    pub device: PathBuf,
    /// Whether the device was made for this, to read the partition
    /// decrypted, and goes again once it is done with.
    mapped: bool,
}

impl Reached {
    /// Take the device away again where it was made for this; the
    /// partition itself stays as it is.
    pub fn close(self) -> Result<()> {
        match self.mapped {
            true => dm::remove(MAPPED),
            false => Ok(()),
        }
    }
}

/// Reach the device the data partition's filesystem is read and written
/// through: the partition mapped decrypted, under the key the phone keeps
/// for it, where an entry of its table has it encrypted whole, and the
/// partition itself where none has.
fn reach(keys: Option<&KeyMint>, entries: &[Entry]) -> Result<Reached> {
    let device = skyward_block::fstab::device(&entries[0])?;
    let Some((entry, encryption)) = encryption::whole(entries)? else {
        return Ok(Reached { device: PathBuf::from(device), mapped: false });
    };
    let key = storage_key(key_service(keys)?, entry, &encryption)?;
    let sectors = skyward_block::size_bytes(&device)? / 512;
    let mapped = dm::create(MAPPED, &[target(&device, sectors, &key, &encryption)], false)?;
    Ok(Reached { device: mapped, mapped: true })
}

/// Reach the data partition's filesystem without mounting it, for it to be
/// repaired. The partition is to be closed, and what this gives is to be
/// closed again once done with.
pub fn reach_unmounted(keys: Option<&KeyMint>) -> Result<Reached> {
    anyhow::ensure!(!mount::is_mount_point(Path::new(MOUNT))? && dm::status(MAPPED)?.is_none(), "the data partition is open");
    reach(keys, &table()?)
}

/// Show the owner's shared storage at /sdcard; shown already, it stays.
fn show_owner_storage() -> Result<()> {
    let at = Path::new(SDCARD);
    if mount::is_mount_point(at)? {
        return Ok(());
    }
    mount::prepare(at)?;
    mount::bind(&shared_storage(), at)
}

/// The owner directory on multi-user Android, or the older shared layout.
pub fn shared_storage() -> PathBuf {
    let owner = Path::new(SHARED_STORAGE).join(OWNER.to_string());
    if owner.is_dir() { owner } else { PathBuf::from(SHARED_STORAGE) }
}

/// Opt-in Qualcomm cryptfs FDE. Authenticate before enabling ICE; the
/// bounded linear view never exposes the footer to a filesystem or fsck.
pub fn open_legacy_fde(operation: &crate::fde::Operation) -> Result<Option<Opening>> {
    use crate::{fde, keymaster3::Keymaster3};
    use std::os::unix::fs::MetadataExt;
    if fde::active() {
        if mount::is_mount_point(Path::new(MOUNT))? {
            anyhow::ensure!(dm::status(MAPPED)?.is_some(), "active cryptfs mount has lost its bounded device");
            show_owner_storage()?;
            return Ok(Some(Opening { mounted: None, closed: Vec::new() }));
        }
        close_locked(operation)?;
    }
    let entries = table()?;
    let entry = entries.iter().find(|e| e.filesystem == "ext4").context("cryptfs needs an ext4 data entry")?;
    let device = PathBuf::from(skyward_block::fstab::device(entry)?);
    let Some(footer) = fde::read_footer(&device)? else { return Ok(None); };
    anyhow::ensure!(!mount::is_mount_point(Path::new(MOUNT))? && dm::status(MAPPED)?.is_none(), "data is already open");
    let binder = Binder::hardware()?;
    let service = Keymaster3::open(&binder)?;
    let derived = footer.derive_default(|blob, input| service.cryptfs_sign(blob, input))?;
    anyhow::ensure!(footer.verify(&derived)?, "cryptfs intermediate-key verification failed");
    let activation = fde::activate(&device, &derived, operation)?;
    let dev = std::fs::metadata(&device)?.rdev();
    let target = dm::Target::Linear { sectors: footer.sectors, device: (libc::major(dev) as u32, libc::minor(dev) as u32), offset: 0 };
    let mapped = dm::create(MAPPED, &[target], false)?;
    let result = (|| -> Result<Opening> {
        mount::prepare(Path::new(MOUNT))?;
        // Confirm ordinary directories while replay and writes are off.
        mount::mount(&mapped, Path::new(MOUNT), "ext4", &["ro".into(), "noload".into(), "nosuid".into(), "nodev".into()])?;
        let media = std::fs::read_dir(SHARED_STORAGE).context("read cryptfs media directory")?;
        for item in media { item.context("read cryptfs media entry")?; }
        mount::umount(Path::new(MOUNT))?;
        // Ordinary ext4 I/O is now encrypted in both directions. A full
        // mount, not a noload remount, lets ext4 replay its own journal.
        let options: Vec<_> = entry.options.iter().filter(|o| !matches!(o.as_str(), "inlinecrypt" | "ro" | "noload")).cloned().collect();
        mount::mount(&mapped, Path::new(MOUNT), "ext4", &options)?;
        show_owner_storage()?;
        Ok(Opening { mounted: Some(Opened { mapped: mapped.clone(), filesystem: "ext4".into(), checked: None }), closed: Vec::new() })
    })();
    match result {
        Ok(opening) => { activation.commit(); Ok(Some(opening)) },
        Err(error) => {
            close_locked(operation).with_context(|| format!("close cryptfs after failed opening: {error:#}"))?;
            Err(error)
        },
    }
}

/// Open direct-mounted FBE with Keymaster 3. The installed mode selects
/// the v1 session keyring or the v2 filesystem keyring independently of
/// the vendor HAL version.
/// Hardware-bound nopassword CE and v3 secdiscardable synthetic-password
/// protectors are supported. Locked users remain closed until Gatekeeper
/// verifies their credential. Replay the journal before inspecting keys,
/// with ordinary directory writes still disabled.
pub fn open_legacy_without_credential() -> Result<Opening> {
    open_legacy_with_mount_selection(false)
}

/// Opt-in selection for kernels supporting both legacy PFK and software FBE.
/// Authenticate DE key storage on a read-only mount before loading user
/// keys; never change options after files have become writable.
pub fn open_legacy_with_mount_selection(automatic: bool) -> Result<Opening> {
    use crate::{fscrypt, keymaster3::Keymaster3};
    let mut entries = skyward_block::fstab::read(MOUNT)?;
    anyhow::ensure!(entries.iter().all(|e| e.extra("metadata_encryption").is_none()), "legacy FBE does not support metadata encryption");
    let newly_mounted = !mount::is_mount_point(Path::new(MOUNT))?;
    let mounted = if newly_mounted {
        let mut opened = None;
        for entry in &entries {
            let device = PathBuf::from(skyward_block::fstab::device(entry)?);
            if skyward_block::probe::filesystem(&device)?.as_deref() != Some(&entry.filesystem) { continue; }
            mount::prepare(Path::new(MOUNT))?;
            let options = crate::plain::mount_options(&entry.options, false);
            mount::mount(&device, Path::new(MOUNT), &entry.filesystem, &options)?;
            opened = Some(Opened { mapped: device, filesystem: entry.filesystem.clone(), checked: None });
            break;
        }
        Some(opened.context("legacy data has no directly mountable filesystem")?)
    } else { None };
    let opened = (|| -> Result<(fscrypt::FileEncryptionMode, Vec<Closed>)> {
        let mode = std::fs::read_to_string(FILE_MODE).with_context(|| format!("read legacy encryption mode {FILE_MODE}"))?;
        let encryption = fscrypt::file_encryption_mode(&mode)?;
        let reference = std::fs::read(SHARED_NAME)?;
        encryption.validate_reference(&reference)?;
        let binder = Binder::hardware()?;
        let service = Keymaster3::open(&binder)?;
        if automatic {
            if let Some(opened) = &mounted {
                let entry = entries.iter_mut().find(|e| e.filesystem == opened.filesystem)
                    .context("legacy data filesystem entry disappeared")?;
                let initial_inline = entry.options.iter().any(|o| o == "inlinecrypt");
                // Unwrap and authenticate the common key once. A hardware
                // failure here is unrelated to inlinecrypt, so do not retry.
                let mut raw = StoredKey::load(Path::new(SHARED_KEY))?
                    .unwrap(Unwrap::Hardware(&service)).context("unwrap common key before legacy mount probe")?;
                let mut current_inline = initial_inline;
                let selected = crate::legacy_mount::probe(initial_inline,
                    mode.trim() == "ice:aes-256-cts:v1",
                    |inline| {
                        if inline != current_inline {
                            // No media bind, user keys, or ordinary writes
                            // exist yet. Never force or lazily unmount.
                            mount::umount(Path::new(MOUNT))?;
                            let selected = crate::legacy_mount::options(&entry.options, inline);
                            mount::mount(&opened.mapped, Path::new(MOUNT), &opened.filesystem,
                                &crate::plain::mount_options(&selected, false))?;
                            current_inline = inline;
                        }
                        Ok(())
                    },
                    || {
                        use crate::legacy_mount::ProbeError;
                        encryption.install(Path::new(MOUNT), &raw, Some(&reference))
                            .map_err(ProbeError::Service)?;
                        let dir = Path::new(USER_KEYS).join("de").join(OWNER.to_string());
                        let stored = StoredKey::load(&dir).map_err(ProbeError::contents)?;
                        // Layout byte '1' alone is not evidence of successful
                        // decryption. Require the hardware-backed GCM tag too.
                        let mut key = stored.unwrap(Unwrap::Hardware(&service)).map_err(ProbeError::Service)?;
                        let valid = key.len() == 64;
                        key.fill(0);
                        if !valid {
                            return Err(ProbeError::Service(anyhow::anyhow!("legacy DE key has an unexpected size")));
                        }
                        Ok(())
                    });
                raw.fill(0);
                let inline = selected?;
                entry.options = crate::legacy_mount::options(&entry.options, inline);
                eprintln!("legacy FBE auto: authenticated DE key with inlinecrypt={inline}; packed version properties retained");
            } else {
                // Other processes may use this mount. Verify normally below,
                // but never switch encryption under a live UI/MTP session.
                eprintln!("legacy FBE auto: keeping existing data mount; close data before changing mount options");
            }
        }
        let mode = encryption;
        let install = |dir: &Path, expected: Option<&[u8]>| -> Result<()> {
            let mut raw = StoredKey::load(dir)?.unwrap(Unwrap::Hardware(&service)).with_context(|| format!("unwrap legacy key in {}", dir.display()))?;
            let result = mode.install(Path::new(MOUNT), &raw, expected);
            raw.fill(0);
            result.map(|_| ())
        };
        install(Path::new(SHARED_KEY), Some(&reference))?;
        let users = users()?;
        anyhow::ensure!(users.contains(&OWNER), "legacy DE key for owner is absent");
        for user in &users {
            install(&Path::new(USER_KEYS).join("de").join(user.to_string()), None)?;
            for prefix in ["user_de", "system_de"] {
                let dir = Path::new(MOUNT).join(prefix).join(user.to_string());
                if dir.is_dir() { anyhow::ensure!(mode.unlocked(&dir)?, "DE key does not open {}", dir.display()); }
            }
        }
        let mut closed = Vec::new();
        for user in users {
            let dir = Path::new(USER_KEYS).join("ce").join(user.to_string()).join("current");
            let stored = StoredKey::load(&dir)?;
            if stored.hardware_bound() {
                install(&dir, None).context("unwrap legacy hardware-bound CE key")?;
                verify_legacy_user(mode, user)?;
            } else {
                let protector = protector(user)?;
                if let Some(lock) = &protector.lock {
                    closed.push(Closed { user, lock: lock.kind, made_up: made_up(user, &protector.id),
                        why: made_up(user, &protector.id).then(|| "legacy FBE does not yet support credentials tied to another user".into()) });
                } else {
                    open_legacy_user_files(&service, mode, user, None)?;
                }
            }
        }
        Ok((mode, closed))
    })();
    let (mode, closed) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            if newly_mounted {
                let cleanup = mount::is_mount_point(Path::new(MOUNT)).and_then(|mounted| {
                    if mounted { mount::umount(Path::new(MOUNT)) } else { Ok(()) }
                });
                if let Err(cleanup) = cleanup {
                    return Err(error.context(format!("unmount legacy data after failed unlock: {cleanup:#}")));
                }
            }
            return Err(error);
        }
    };
    if let Some(opened) = &mounted {
        // v1 keys survive a fresh mount via the session keyring. v2 keys
        // belong to this filesystem instance and must survive the switch
        // to writable operation without a full unmount.
        let entry = entries.iter().find(|e| e.filesystem == opened.filesystem).context("legacy data filesystem entry disappeared")?;
        match mode {
            fscrypt::FileEncryptionMode::V1 => {
                mount::umount(Path::new(MOUNT))?;
                mount::mount(&opened.mapped, Path::new(MOUNT), &opened.filesystem, &crate::plain::mount_options(&entry.options, true))?;
            }
            fscrypt::FileEncryptionMode::V2 => {
                // v2 keys belong to this superblock. A full unmount would
                // evict them, unlike the inherited v1 session keyring.
                // The initial mount replayed the journal and stripped
                // noload/norecovery, so this remount keeps journaling live.
                mount::remount_writable(Path::new(MOUNT), true)?;
            }
        }
    }
    if !closed.iter().any(|c| c.user == OWNER) {
        anyhow::ensure!(shared_storage().is_dir(), "legacy data has no existing shared storage");
        show_owner_storage()?;
    }
    Ok(Opening { mounted, closed })
}

fn verify_legacy_user(mode: crate::fscrypt::FileEncryptionMode, user: u32) -> Result<()> {
    for prefix in ["user", "system_ce", "media"] {
        let dir = Path::new(MOUNT).join(prefix).join(user.to_string());
        if dir.is_dir() { anyhow::ensure!(mode.unlocked(&dir)?, "CE key does not open {}", dir.display()); }
    }
    Ok(())
}

/// The old and new stores coexist on upgraded systems. An existing
/// keystore2 database is authoritative; do not hide its errors by falling
/// back to possibly stale file-backed keys.
fn legacy_protector_key(alias: &str) -> Result<Vec<u8>> {
    let database = Path::new(crate::keystore2::DATABASE);
    match std::fs::metadata(database) {
        Ok(_) => crate::keystore2::key_blob(database, alias),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound =>
            crate::keystore1::key_blob(Path::new("/data/misc/keystore"), alias),
        Err(e) => Err(e).context("inspect the phone's keystore database"),
    }
}

fn open_legacy_user_files(service: &crate::keymaster3::Keymaster3, mode: crate::fscrypt::FileEncryptionMode,
    user: u32, credential: Option<&[u8]>) -> Result<()> {
    let protector = protector(user)?;
    anyhow::ensure!(!made_up(user, &protector.id), "legacy FBE does not yet support a credential tied to another user");
    anyhow::ensure!(matches!(&protector.bound, Bound::Secdiscardable(_)), "legacy FBE requires unsupported Weaver authentication");
    // Validate the supported format and fetch the key before attempting
    // authentication. One submitted credential causes one Gatekeeper try.
    crate::user::content(&protector.blob)?;
    let held = legacy_protector_key(&protector.key_alias())?;
    let mut stretched = match &protector.lock {
        Some(lock) => lock.stretched(credential.context("legacy FBE requires the screen-lock credential")?)?,
        None => crate::user::stretched_without_credential(),
    };
    let recovered = (|| -> Result<Vec<u8>> {
        let token = match &protector.lock {
            Some(lock) => {
                let (binder, hardware) = (Binder::open()?, Binder::hardware()?);
                let mut password = crate::user::gatekeeper_password(&stretched);
                let verified = Gatekeeper::open(&binder, &hardware)?.verify(
                    TYPED_SLOT.checked_add(user).context("Gatekeeper user slot overflow")?, &lock.enrolled, &password);
                password.fill(0);
                Some(verified?)
            }
            None => None,
        };
        crate::user::file_secret_with_stretched(&protector, &stretched, |nonce, body| {
            service.gcm_decrypt_authenticated(&held, &[], nonce, body, token.as_ref())
        }).context("recover legacy CE synthetic-password secret")
    })();
    stretched.fill(0);
    let mut secret = recovered?;
    let dir = Path::new(USER_KEYS).join("ce").join(user.to_string()).join("current");
    let unwrapped = StoredKey::load(&dir).and_then(|stored| stored.unwrap(Unwrap::<crate::keymaster3::Keymaster3>::Secret(&secret)));
    secret.fill(0);
    let mut raw = unwrapped.with_context(|| format!("unwrap legacy CE key in {}", dir.display()))?;
    let installed = mode.install(Path::new(MOUNT), &raw, None);
    raw.fill(0);
    installed?;
    verify_legacy_user(mode, user)
}

/// Unlock a user on the opt-in Keymaster 3 path. Keep other users closed
/// and publish owner storage only after its CE policy really opens.
pub fn unlock_legacy(closed: &[Closed], user: u32, credential: &[u8]) -> Result<Vec<Closed>> {
    anyhow::ensure!(closed.iter().any(|c| c.user == user && !c.made_up), "user is not awaiting a legacy credential");
    anyhow::ensure!(mount::is_mount_point(Path::new(MOUNT))?, "legacy data is not mounted");
    let mode = crate::fscrypt::file_encryption_mode(&std::fs::read_to_string(FILE_MODE)?)?;
    let binder = Binder::hardware()?;
    let service = crate::keymaster3::Keymaster3::open(&binder)?;
    open_legacy_user_files(&service, mode, user, Some(credential))?;
    if user == OWNER {
        anyhow::ensure!(shared_storage().is_dir(), "legacy data has no existing shared storage");
        show_owner_storage()?;
    }
    Ok(closed.iter().filter(|c| c.user != user).cloned().collect())
}

/// Open only demonstrably plain data without contacting KeyMint. Legacy
/// full-disk encryption and FBE still require their own key services.
/// Initialize media only on an empty filesystem without encryption policies.
pub fn open_without_keys() -> Result<Opening> {
    let entries = skyward_block::fstab::read(MOUNT)?;
    anyhow::ensure!(!entries.is_empty(), "the phone's table names no data filesystem");
    anyhow::ensure!(!entries.iter().any(|e| e.extra("metadata_encryption").is_some()),
        "data is configured for metadata encryption and needs a key service");
    let newly_mounted = !mount::is_mount_point(Path::new(MOUNT))?;
    let mounted = if newly_mounted {
        let mut opened = None;
        for entry in &entries {
            let device = PathBuf::from(skyward_block::fstab::device(entry)?);
            if skyward_block::probe::filesystem(&device)?.as_deref() != Some(&entry.filesystem) { continue; }
            mount::prepare(Path::new(MOUNT))?;
            // Let ext4 replay committed metadata before inspecting paths.
            // noload can expose stale directory entries after a reset,
            // including media pointing to an inode not yet recovered.
            let options = crate::plain::mount_options(&entry.options, false);
            mount::mount(&device, Path::new(MOUNT), &entry.filesystem, &options)?;
            opened = Some(Opened { mapped: device, filesystem: entry.filesystem.clone(), checked: None });
            break;
        }
        Some(opened.context("data holds no directly mountable filesystem; legacy encrypted data needs decryption support")?)
    } else { None };
    let opened = (|| -> Result<()> {
        let file_encryption = entries.iter().any(|e| e.extra("fileencryption").is_some());
        let mut initialize = crate::plain::inspect(Path::new(MOUNT), file_encryption, ensure_plain)?;
        if let Some(opened) = &mounted {
            // A writable remount retains ext4's noload/norecovery option.
            // Reopen normally so subsequent writes have a working journal
            // and use the device table's nosuid/nodev and other options.
            let entry = entries.iter().find(|e| e.filesystem == opened.filesystem).context("data filesystem entry disappeared")?;
            mount::umount(Path::new(MOUNT))?;
            mount::mount(&opened.mapped, Path::new(MOUNT), &opened.filesystem, &crate::plain::mount_options(&entry.options, true))?;
            // Journal replay must not turn an apparently plain layout into
            // a reported unlock of encrypted data.
            initialize = crate::plain::inspect(Path::new(MOUNT), file_encryption, ensure_plain)?;
        }
        if initialize {
            use std::os::unix::fs::PermissionsExt;
            for dir in [SHARED_STORAGE.to_string(), format!("{SHARED_STORAGE}/{OWNER}")] {
                std::fs::create_dir(&dir).with_context(|| format!("initialize {dir} after formatting data"))?;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o770))?;
                std::os::unix::fs::chown(&dir, Some(1023), Some(1023)).with_context(|| format!("set media ownership of {dir}"))?;
            }
        }
        show_owner_storage()
    })();
    if let Err(error) = opened {
        if newly_mounted && mount::is_mount_point(Path::new(MOUNT))? {
            mount::umount(Path::new(MOUNT)).context("unmount data after failed plain opening")?;
        }
        return Err(error);
    }
    Ok(Opening { mounted, closed: Vec::new() })
}

#[cfg(target_os = "android")]
fn ensure_plain(dir: &Path) -> Result<()> {
    use std::os::fd::AsRawFd;
    // The v1 policy ioctl is also understood by Android's older kernels.
    // Linux defines this GET ioctl with _IOW, despite its name.
    const GET_POLICY: u32 = 0x400c_6615;
    let file = std::fs::File::open(dir)?;
    let mut policy = [0u8; 12];
    let result = unsafe { libc::ioctl(file.as_raw_fd(), GET_POLICY as libc::Ioctl, policy.as_mut_ptr()) };
    anyhow::ensure!(result != 0, "{} has an encryption policy and needs keys", dir.display());
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENODATA | libc::EOPNOTSUPP | libc::ENOTTY) => Ok(()),
        Some(libc::ENOENT) => {
            // Old ext4_get_policy returns ENOENT whenever reading its
            // encryption context fails, including malformed contexts. Check
            // the inode flag on the same open descriptor before accepting it
            // as policy-free; never reinterpret File::open's ENOENT here.
            const GET_FLAGS: u32 = 0x8000_6601 | ((std::mem::size_of::<libc::c_long>() as u32) << 16);
            let mut flags: libc::c_long = 0;
            let result = unsafe { libc::ioctl(file.as_raw_fd(), GET_FLAGS as libc::Ioctl, &mut flags) };
            if result != 0 {
                return Err(std::io::Error::last_os_error())
                    .with_context(|| format!("inspect inode flags of {} after missing encryption policy", dir.display()));
            }
            crate::plain::ensure_unencrypted_flags(flags as u32)
                .with_context(|| format!("inspect encryption of {}", dir.display()))
        }
        _ => Err(error).with_context(|| format!("inspect encryption of {}", dir.display())),
    }
}

#[cfg(not(target_os = "android"))]
fn ensure_plain(_dir: &Path) -> Result<()> {
    anyhow::bail!("data encryption is inspected only on Android")
}

/// Close what opening the partition opened, where the recovery opened it:
/// /sdcard, the partition's mount, and the device it is read through.
pub fn close() -> Result<()> {
    let operation = crate::fde::lock()?;
    close_locked(&operation)
}

fn close_locked(operation: &crate::fde::Operation) -> Result<()> {
    for at in [SDCARD, MOUNT] {
        if mount::is_mount_point(Path::new(at))? {
            mount::umount(Path::new(at))?;
        }
    }
    if dm::status(MAPPED)?.is_some() {
        dm::remove(MAPPED)?;
    }
    crate::fde::deactivate(operation)?;
    Ok(())
}

/// A user whose own files are still closed: who, the kind of screen lock
/// they set in the phone's numbering, and whether the phone made their
/// credential up, in which case they open behind the owner and nobody is
/// asked for it. `why` says why such a user did not open behind the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed {
    pub user: u32,
    pub lock: i32,
    pub made_up: bool,
    pub why: Option<String>,
}

/// The data partition as far as it opens with nothing a user knows.
pub struct Opening {
    /// How it was mounted, where this mounted it rather than an earlier
    /// run of the recovery.
    pub mounted: Option<Opened>,
    /// The users whose own files are still closed, the owner first.
    pub closed: Vec<Closed>,
}

/// Open the data partition as far as it opens with nothing a user knows:
/// mount it where it is not mounted, let in the keys of the files that
/// belong to no one user and of each user's files readable before they
/// sign in, and open the own files of every user who set no screen lock
/// and, behind the owner's, of those whose credential the phone made up.
/// `keys` is the phone's key service, which a partition the phone's table
/// has encrypted in neither way is opened without.
pub fn open_without_credential(keys: Option<&KeyMint>) -> Result<Opening> {
    let entries = table()?;
    let mounted = match mount::is_mount_point(Path::new(MOUNT))? {
        true => None,
        false => Some(open(keys, &entries)?),
    };
    // A phone whose table has no file of the partition encrypted reads
    // every name and every file as it is: there is no key to let in, and
    // nobody's own files stay closed. The owner's shared storage shows
    // once the system has made it, which it has not on a partition made
    // afresh that the phone has not started from yet.
    if !encryption::files(&entries) {
        if Path::new(SHARED_STORAGE).join(OWNER.to_string()).is_dir() {
            show_owner_storage()?;
        }
        return Ok(Opening { mounted, closed: Vec::new() });
    }
    let service = key_service(keys)?;
    open_shared(service)?;
    let mut closed = Vec::new();
    let mut owner = Note::default();
    for user in open_users(service)? {
        let protector = protector(user)?;
        match protector.lock {
            None => {
                let note = open_user_files(service, user, None, None)?;
                if user == OWNER {
                    owner = note;
                }
            }
            Some(lock) => closed.push(Closed { user, lock: lock.kind, made_up: made_up(user, &protector.id), why: None }),
        }
    }
    Ok(Opening { mounted, closed: open_made_up(service, closed, &owner) })
}

/// Open the own files of `user` with what they typed at their screen lock
/// and, where `user` is the owner, of each of `closed` whose credential
/// the phone made up behind theirs; give those still closed.
pub fn unlock(service: &KeyMint, closed: &[Closed], user: u32, credential: &[u8]) -> Result<Vec<Closed>> {
    let note = open_user_files(service, user, Some(credential), None)?;
    let rest = closed.iter().filter(|c| c.user != user).cloned().collect();
    Ok(match user == OWNER {
        true => open_made_up(service, rest, &note),
        false => rest,
    })
}

/// Open the own files of each of `closed` whose credential the phone made
/// up, behind `owner`, the note the owner's own secret gave; give those
/// still closed, a made-up one with why it did not open. Where the secure
/// world wrote no note, none of them is tried and each is left with why.
fn open_made_up(service: &KeyMint, closed: Vec<Closed>, owner: &Note) -> Vec<Closed> {
    let Some(token) = &owner.token else {
        return closed
            .into_iter()
            .map(|c| match c.made_up {
                true => Closed { why: owner.refused.clone(), ..c },
                false => c,
            })
            .collect();
    };
    closed
        .into_iter()
        .filter_map(|c| match c.made_up {
            false => Some(c),
            true => match open_user_files(service, c.user, None, Some(token)) {
                Ok(_) => None,
                Err(e) => Some(Closed { why: Some(format!("{e:#}")), ..c }),
            },
        })
        .collect()
}

/// Whether the phone made up the credential of `user`, whose protector is
/// `id`, keeping it behind the owner's.
fn made_up(user: u32, id: &str) -> bool {
    made_up_record(user, id).is_some()
}

/// Where the phone keeps the key for the files that are readable before
/// anyone signs in, and what it says about how those files are encrypted.
const SHARED_KEY: &str = "/data/unencrypted/key";
const SHARED_NAME: &str = "/data/unencrypted/ref";
const FILE_MODE: &str = "/data/unencrypted/mode";

/// Where the phone keeps each user's own keys.
const USER_KEYS: &str = "/data/misc/vold/user_keys";

/// Let in the key for the files that belong to no one user. Until it is
/// in, most of what the data partition holds reads as scrambled names.
fn open_shared(service: &KeyMint) -> Result<[u8; crate::fscrypt::NAME_BYTES]> {
    let mode = std::fs::read_to_string(FILE_MODE).with_context(|| format!("read {FILE_MODE}"))?;
    let wrapped = crate::fscrypt::hardware_wrapped(mode.trim());
    let raw = hardware_key(service, Path::new(SHARED_KEY), wrapped)?;
    let name = crate::fscrypt::add_key(Path::new(MOUNT), &raw, wrapped)?;
    let recorded = crate::fscrypt::recorded_name(Path::new(SHARED_NAME))?;
    if name != recorded {
        anyhow::bail!("the key in {SHARED_KEY} is not the one {SHARED_NAME} names");
    }
    Ok(name)
}

/// Which users the phone keeps keys for, in order.
fn users() -> Result<Vec<u32>> {
    let dir = Path::new(USER_KEYS).join("de");
    let mut users = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry.with_context(|| format!("read {}", dir.display()))?.path();
        let Some(user) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u32>().ok()) else { continue };
        users.push(user);
    }
    users.sort_unstable();
    Ok(users)
}

/// Let in the keys for the files of each user that are readable before
/// that user signs in, and give the users they belong to.
fn open_users(service: &KeyMint) -> Result<Vec<u32>> {
    let mode = std::fs::read_to_string(FILE_MODE).with_context(|| format!("read {FILE_MODE}"))?;
    let wrapped = crate::fscrypt::hardware_wrapped(mode.trim());
    let users = users()?;
    for user in &users {
        let dir = Path::new(USER_KEYS).join("de").join(user.to_string());
        let raw = hardware_key(service, &dir, wrapped)?;
        crate::fscrypt::add_key(Path::new(MOUNT), &raw, wrapped)?;
    }
    Ok(users)
}

/// Where a phone keeps what it needs to recover a user's own secret.
const PROTECTORS: &str = "/data/system_de";

/// Read what a user's own credential keeps their secret behind. A phone
/// may keep more than one blob for a user: Android keeps one for each
/// token some part of the phone was given to reach the secret with, and
/// HyperOS, once the owner changes one screen lock for another with a
/// fingerprint enrolled, keeps a second one for a while, so that the
/// credential before can still be used. Which of them the user's current
/// credential opens is the phone's own business, and it writes that down,
/// so this reads it rather than guess.
fn protector(user: u32) -> Result<crate::user::Protector> {
    let dir = blobs(user);
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry.with_context(|| format!("read {}", dir.display()))?.path();
        if path.extension().is_some_and(|e| e == "spblob")
            && let Some(id) = path.file_stem().and_then(|s| s.to_str())
        {
            ids.push(id.to_string());
        }
    }
    let id = match crate::locksettings::current_protector(Path::new(crate::locksettings::DATABASE), user)? {
        Some(recorded) => {
            anyhow::ensure!(
                ids.contains(&recorded),
                "the phone keeps user {user}'s secret behind the blob {recorded}, which is not among the {} in {}",
                ids.len(),
                dir.display()
            );
            recorded
        }
        // Where the phone records none, only a blob that is alone is
        // beyond doubt the user's.
        None => match &ids[..] {
            [only] => only.clone(),
            _ => anyhow::bail!(
                "{} holds {} of the user's blobs, and the phone records none of them as the one the user's own credential opens",
                dir.display(),
                ids.len()
            ),
        },
    };
    // A user who has set a screen lock has what the phone recorded about
    // it beside the blob; a user who has set none has nothing there.
    let recorded = dir.join(format!("{id}.pwd"));
    let lock = match recorded.exists() {
        true => Some(crate::user::Lock::of(
            &std::fs::read(&recorded).with_context(|| format!("read {}", recorded.display()))?,
        )?),
        false => None,
    };
    // A phone with a weaver records the slot it keeps for the blob; any
    // other keeps the bytes the blob is bound to beside it.
    let slot = dir.join(format!("{id}.weaver"));
    let bound = match slot.exists() {
        true => Bound::Weaver(crate::user::weaver_slot(&std::fs::read(&slot).with_context(|| format!("read {}", slot.display()))?)?),
        false => Bound::Secdiscardable(
            std::fs::read(dir.join(format!("{id}.secdis"))).with_context(|| format!("read what the blob in {} is bound to", dir.display()))?,
        ),
    };
    Ok(crate::user::Protector {
        blob: std::fs::read(dir.join(format!("{id}.spblob"))).with_context(|| format!("read the blob in {}", dir.display()))?,
        bound,
        id,
        lock,
    })
}

/// Which of the secure world's slots a user's enrolments sit in. What the
/// user types is enrolled a hundred thousand above their own number,
/// keeping it apart from the enrolments made before phones kept
/// credentials this way; the secret behind it is enrolled under the
/// number itself.
const TYPED_SLOT: u32 = 100_000;

/// What the phone calls the enrolment of a user's own secret, among the
/// blobs it keeps for that user.
const BLOBS: &str = "spblob";
const SECRET_HANDLE: &str = "0000000000000000.handle";

/// Where a user's blobs are.
fn blobs(user: u32) -> PathBuf {
    Path::new(PROTECTORS).join(user.to_string()).join(BLOBS)
}

/// Where a phone keeps the credential it made up itself for a user whose
/// screen lock is the owner's. The file holds a nonce and then the
/// credential, encrypted under a key the owner's own secret guards.
const MADE_UP: &str = "/data/system/users";
const MADE_UP_FILE: &str = "gatekeeper.profile.key";

/// The name the key service keeps that key under, the user's number after
/// it.
const MADE_UP_KEY: &str = "profile_key_name_decrypt_";

/// HyperOS keeps the file beside the blob of the user's protector instead,
/// named after the protector with this ending, and puts the protector's
/// name after the user's number in the key's name too.
const MADE_UP_BESIDE_BLOB: &str = "profile_pwd";

/// The file holding the credential the phone made up for `user`, whose
/// protector is `id`, and the name of the key it is encrypted under: where
/// Android keeps them, or where HyperOS does. `None` where it made none up.
fn made_up_record(user: u32, id: &str) -> Option<(PathBuf, String)> {
    let android = Path::new(MADE_UP).join(user.to_string()).join(MADE_UP_FILE);
    if android.is_file() {
        return Some((android, format!("{MADE_UP_KEY}{user}")));
    }
    let hyperos = blobs(user).join(format!("{id}.{MADE_UP_BESIDE_BLOB}"));
    hyperos.is_file().then(|| (hyperos, format!("{MADE_UP_KEY}{user}.{id}")))
}

/// The credential a phone made up for `user`, whose protector is `id`,
/// where it made one up. A phone does this for the users it keeps beside
/// the owner's own, such as the one that holds a second copy of an app:
/// nobody ever types their credential, so the phone writes one down for
/// itself and keeps it behind the owner's. `owner` is the note the owner's
/// credential gave.
fn made_up_credential(service: &KeyMint, user: u32, id: &str, owner: Option<&Token>) -> Result<Option<Vec<u8>>> {
    let Some((file, alias)) = made_up_record(user, id) else { return Ok(None) };
    let stored = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
    if stored.len() < NONCE_BYTES + TAG_BYTES {
        anyhow::bail!("{} is {} bytes, too few to hold a credential", file.display(), stored.len());
    }
    let (nonce, body) = stored.split_at(NONCE_BYTES);
    let held = crate::keystore2::key_blob(Path::new(crate::keystore2::DATABASE), &alias)?;
    let credential = service.gcm_decrypt(&held, &[], nonce, body, owner).with_context(|| {
        format!("open the credential the phone made up for user {user}, which is kept behind the owner's own")
    })?;
    Ok(Some(credential))
}

/// How long the nonce and the check are on everything the key service
/// encrypts.
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;

/// Let in the key for a user's own files, and give the note the secure
/// world wrote for the secret behind their credential, which is what the
/// keys the phone keeps for them are guarded by. `credential` is what the
/// user would type at their screen lock; a user who has set none needs
/// none, and a user the phone made one up for needs `owner`, the note the
/// owner's own secret gave. The owner's files open, their shared storage
/// shows at /sdcard.
fn open_user_files(service: &KeyMint, user: u32, credential: Option<&[u8]>, owner: Option<&Token>) -> Result<Note> {
    let protector = protector(user)?;
    let stretched = match &protector.lock {
        Some(lock) => {
            let typed = match made_up_credential(service, user, &protector.id, owner)? {
                Some(made_up) => made_up,
                None => credential
                    .with_context(|| format!("user {user} has set a screen lock, and their files do not open without it"))?
                    .to_vec(),
            };
            lock.stretched(&typed).context("stretch the credential")?
        }
        None => crate::user::stretched_without_credential(),
    };
    let (secret, note) = match (&protector.bound, &protector.lock) {
        // The weaver checks the credential itself, and gives back the value
        // the secret is bound to only for the right one. The key holding
        // the secret is then guarded by nothing else.
        (Bound::Weaver(slot), _) => {
            let binder = Binder::open()?;
            let weaver = Weaver::open(&binder)?;
            let key = crate::user::weaver_key(&stretched, weaver.config()?.key_bytes)?;
            let value = weaver.read(*slot, &key)?;
            (crate::user::weaver_protector_secret(&stretched, &value), None)
        }
        // The key holding this user's secret is guarded by the screen lock,
        // so the secure world has to be shown the credential itself before
        // it will use that key.
        (Bound::Secdiscardable(secdiscardable), Some(lock)) => {
            let (binder, hardware) = (Binder::open()?, Binder::hardware()?);
            let note = Gatekeeper::open(&binder, &hardware)?.verify(
                TYPED_SLOT + user,
                &lock.enrolled,
                &crate::user::gatekeeper_password(&stretched),
            )?;
            (crate::user::protector_secret(&stretched, secdiscardable), Some(note))
        }
        (Bound::Secdiscardable(secdiscardable), None) => (crate::user::protector_secret(&stretched, secdiscardable), None),
    };
    let held = crate::keystore2::key_blob(Path::new(crate::keystore2::DATABASE), &protector.key_alias())?;
    let content = crate::user::content(&protector.blob)?;
    if content.len() < NONCE_BYTES + TAG_BYTES {
        anyhow::bail!("the user's blob is too short to hold anything");
    }
    let (nonce, body) = content.split_at(NONCE_BYTES);
    let inner = service
        .gcm_decrypt(&held, &[], nonce, body, note.as_ref())
        .context("open what the secure world holds the user's blob closed with")?;
    let synthetic_password = crate::user::open_inner(&secret, &inner)?;
    let file_secret = crate::user::file_secret(&synthetic_password);

    let mode = std::fs::read_to_string(FILE_MODE).with_context(|| format!("read {FILE_MODE}"))?;
    let wrapped = crate::fscrypt::hardware_wrapped(mode.trim());
    let dir = Path::new(USER_KEYS).join("ce").join(user.to_string()).join("current");
    let stored = StoredKey::load(&dir).with_context(|| format!("read the key in {}", dir.display()))?;
    let raw = stored.unwrap(Unwrap::<KeyMint>::Secret(&file_secret)).with_context(|| format!("unwrap the key in {}", dir.display()))?;
    let raw = match wrapped {
        true => service.ephemeral_storage_key(&raw).with_context(|| format!("make the key in {} usable now", dir.display()))?,
        false => raw,
    };
    crate::fscrypt::add_key(Path::new(MOUNT), &raw, wrapped)?;
    if user == OWNER {
        show_owner_storage()?;
    }
    secret_note(user, &synthetic_password)
}

/// What the secure world wrote for a user's own secret, which is what the
/// credentials the phone made up for other users are kept behind. A user
/// whose secret the secure world knows nothing about has none, and so has
/// one whose secure world would write none; the user's own files are open
/// either way, and only the users kept behind this one stay closed.
#[derive(Debug, Default)]
struct Note {
    token: Option<Token>,
    /// Why the secure world wrote none, where it was asked and would not.
    refused: Option<String>,
}

/// Have the secure world check the user's own secret, so the keys the
/// phone keeps for that user can be used.
fn secret_note(user: u32, synthetic_password: &[u8]) -> Result<Note> {
    let handle = blobs(user).join(SECRET_HANDLE);
    if !handle.is_file() {
        return Ok(Note::default());
    }
    let enrolled = std::fs::read(&handle).with_context(|| format!("read {}", handle.display()))?;
    let (binder, hardware) = (Binder::open()?, Binder::hardware()?);
    let password = crate::user::secret_gatekeeper_password(synthetic_password);
    let asked = Gatekeeper::open(&binder, &hardware)
        .and_then(|gatekeeper| gatekeeper.verify(user, &enrolled, &password))
        .with_context(|| format!("have the secure world check the secret behind user {user}'s credential"));
    Ok(match asked {
        Ok(token) => Note { token: Some(token), refused: None },
        Err(e) => Note { token: None, refused: Some(format!("{e:#}")) },
    })
}
