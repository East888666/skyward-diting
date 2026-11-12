//! USB drives plugged into the phone's port: found among the kernel's
//! disks by the USB bus they hang from, and shown read only for the files
//! on them to be read.

use crate::mount;
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path};

/// Where the kernel lists its disks, each a link to the device it is.
const SYS_BLOCK: &str = "/sys/block";

/// Where ueventd makes the nodes of disks and partitions.
const DEV_BLOCK: &str = "/dev/block";

/// Statically linked, so the installed ROM cannot change its libraries.
const NTFS_HELPER: &str = "/skyward/ntfs-3g";

/// The filesystems a USB drive may hold, in the order they are tried, with
/// what each is mounted with to give names as the drive keeps them: FAT
/// and exFAT keep long names in UTF-16, which the kernel otherwise turns
/// into a code page without Chinese.
const FILESYSTEMS: [(&str, &str); 5] =
    [("vfat", "utf8"), ("exfat", "iocharset=utf8"), ("ntfs3", "iocharset=utf8"), ("ext4", ""), ("f2fs", "")];

#[derive(Debug, PartialEq, Eq)]
enum Volume {
    Ntfs,
    VentoyEfi,
    Other,
}

fn volume(boot: &[u8]) -> Volume {
    if boot.len() < 512 || boot[510..512] != [0x55, 0xaa] {
        return Volume::Other;
    }
    if &boot[3..11] == b"NTFS    " {
        return Volume::Ntfs;
    }
    // Ventoy's small second partition holds boot files. A readable boot
    // partition must not conceal a data partition that failed to mount.
    if (&boot[54..62] == b"FAT16   " && &boot[43..54] == b"VTOYEFI    ")
        || (&boot[82..90] == b"FAT32   " && &boot[71..82] == b"VTOYEFI    ")
    {
        return Volume::VentoyEfi;
    }
    Volume::Other
}

fn probe(device: &Path) -> Result<Volume> {
    let mut boot = Vec::with_capacity(512);
    fs::File::open(device)
        .and_then(|f| f.take(512).read_to_end(&mut boot))
        .with_context(|| format!("read the boot sector of {}", device.display()))?;
    Ok(volume(&boot))
}

/// A USB drive as the kernel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// The disk's name, such as `sdg`.
    pub disk: String,
    /// Its partitions by name, each with its size in bytes.
    pub parts: Vec<(String, u64)>,
}

impl Drive {
    /// Whether ueventd has made the nodes of the disk and its partitions,
    /// which come a moment after the kernel lists them.
    pub fn ready(&self) -> bool {
        std::iter::once(&self.disk).chain(self.parts.iter().map(|(name, _)| name)).all(|name| Path::new(DEV_BLOCK).join(name).exists())
    }

    /// What to show of the drive, in the order it is tried: its partitions,
    /// the largest first, or the whole disk for a drive formatted without
    /// a partition table.
    fn candidates(&self) -> Vec<&str> {
        if self.parts.is_empty() {
            return vec![&self.disk];
        }
        let mut parts: Vec<&(String, u64)> = self.parts.iter().collect();
        parts.sort_by_key(|&(_, size)| std::cmp::Reverse(*size));
        parts.into_iter().map(|(name, _)| name.as_str()).collect()
    }
}

/// The USB drives plugged in that hold anything, by the names of their
/// disks. A drive pulled out while it is looked at is left out.
pub fn drives() -> Result<Vec<Drive>> {
    drives_in(Path::new(SYS_BLOCK))
}

fn drives_in(listed: &Path) -> Result<Vec<Drive>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(listed).with_context(|| format!("read {}", listed.display()))? {
        let link = entry.with_context(|| format!("read {}", listed.display()))?.path();
        let Some(device) = present(fs::canonicalize(&link)).with_context(|| format!("follow {}", link.display()))? else { continue };
        if !on_usb(&device) {
            continue;
        }
        // A card reader with no card in it is a disk of no size.
        let Some(size) = sectors(&device)? else { continue };
        if size == 0 {
            continue;
        }
        let Some(inside) = present(fs::read_dir(&device)).with_context(|| format!("read {}", device.display()))? else { continue };
        let mut parts = Vec::new();
        for entry in inside {
            let dir = entry.with_context(|| format!("read {}", device.display()))?.path();
            if !dir.join("partition").is_file() {
                continue;
            }
            let Some(size) = sectors(&dir)? else { continue };
            parts.push((name(&dir)?, size * 512));
        }
        parts.sort();
        found.push(Drive { disk: name(&link)?, parts });
    }
    found.sort_by(|a, b| a.disk.cmp(&b.disk));
    Ok(found)
}

/// Whether a disk's device hangs from a USB bus, `usb1` and the like, as a
/// drive plugged into the port does; the phone's own storage hangs from
/// its UFS controller.
fn on_usb(device: &Path) -> bool {
    device.components().any(|part| match part {
        Component::Normal(name) => name
            .to_str()
            .and_then(|n| n.strip_prefix("usb"))
            .is_some_and(|bus| !bus.is_empty() && bus.bytes().all(|b| b.is_ascii_digit())),
        _ => false,
    })
}

/// The size the kernel gives a disk or partition at `dir`, in 512-byte
/// sectors; none once it is gone.
fn sectors(dir: &Path) -> Result<Option<u64>> {
    let path = dir.join("size");
    let Some(text) = present(fs::read_to_string(&path)).with_context(|| format!("read {}", path.display()))? else { return Ok(None) };
    let sectors = text.trim().parse().with_context(|| format!("{} holds {text:?}", path.display()))?;
    Ok(Some(sectors))
}

fn name(path: &Path) -> Result<String> {
    Ok(path.file_name().with_context(|| format!("{} has no name", path.display()))?.to_string_lossy().into_owned())
}

/// What `r` holds, or nothing for what was gone when it was looked at.
fn present<T>(r: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Show `drive` read only at `at`: the first of its partitions, or its
/// disk, that holds a readable filesystem. NTFS can use the bundled FUSE
/// reader when the kernel has no driver. Gives the partition and filesystem.
pub fn mount(drive: &Drive, at: &Path) -> Result<(String, &'static str)> {
    fs::create_dir_all(at).with_context(|| format!("create {}", at.display()))?;
    mount_using(drive, Path::new(DEV_BLOCK), |device, filesystem, own| {
        let options: Vec<String> = ["ro", "nosuid", "nodev", "noexec", own].map(str::to_string).into();
        mount::mount(device, at, filesystem, &options)
    }, |device| {
        let helper = Path::new(NTFS_HELPER);
        anyhow::ensure!(helper.is_file(), "this kernel cannot mount NTFS; rebuild the recovery with --ntfs3g (see tools/build-ntfs3g.sh)");
        let args = ["-o", "ro,nosuid,nodev,noexec,norecover", device.to_str().context("USB device path is not UTF-8")?, at.to_str().context("USB mount path is not UTF-8")?];
        let out = std::process::Command::new(helper).args(args).output().context("run the NTFS reader")?;
        anyhow::ensure!(out.status.success(), "NTFS reader ended with {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
        anyhow::ensure!(mount::is_mount_point(at)?, "NTFS reader did not mount {}", device.display());
        Ok(())
    })
}

fn mount_using(
    drive: &Drive,
    devices: &Path,
    mut kernel: impl FnMut(&Path, &'static str, &str) -> Result<()>,
    mut ntfs: impl FnMut(&Path) -> Result<()>,
) -> Result<(String, &'static str)> {
    let mut tried = Vec::new();
    for name in drive.candidates() {
        let device = devices.join(name);
        let volume = match probe(&device) {
            Ok(volume) => volume,
            Err(e) => {
                tried.push(format!("{name}: {e:#}"));
                continue;
            }
        };
        if volume == Volume::VentoyEfi && drive.parts.len() > 1 {
            tried.push(format!("{name}: skipped Ventoy boot partition VTOYEFI"));
            continue;
        }
        let filesystems: &[(&str, &str)] = if volume == Volume::Ntfs {
            &[("ntfs3", "iocharset=utf8"), ("ntfs", "utf8")]
        } else {
            &FILESYSTEMS
        };
        for &(filesystem, own) in filesystems {
            match kernel(&device, filesystem, own) {
                Ok(()) => return Ok((name.to_string(), filesystem)),
                Err(e) => tried.push(format!("{name} as {filesystem}: {}", e.root_cause())),
            }
        }
        if volume == Volume::Ntfs {
            match ntfs(&device) {
                Ok(()) => return Ok((name.to_string(), "ntfs-3g")),
                Err(e) => tried.push(format!("{name} as ntfs-3g: {e:#}")),
            }
        }
    }
    bail!("{} has no readable data filesystem: {}", drive.disk, tried.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A disk at `path` under `root`'s devices, with `sectors` and these
    /// partitions, each with its sectors, listed in `root`'s block folder
    /// as the kernel lists its disks.
    fn disk(root: &Path, path: &str, sectors: u64, parts: &[(&str, u64)]) {
        let dir = root.join("devices").join(path);
        fs::create_dir_all(dir.join("queue")).unwrap();
        fs::write(dir.join("size"), format!("{sectors}\n")).unwrap();
        for (k, (part, sectors)) in parts.iter().enumerate() {
            fs::create_dir_all(dir.join(part)).unwrap();
            fs::write(dir.join(part).join("partition"), format!("{}\n", k + 1)).unwrap();
            fs::write(dir.join(part).join("size"), format!("{sectors}\n")).unwrap();
        }
        let name = dir.file_name().unwrap().to_owned();
        fs::create_dir_all(root.join("block")).unwrap();
        symlink(&dir, root.join("block").join(name)).unwrap();
    }

    #[test]
    fn only_disks_on_a_usb_bus_that_hold_something_are_drives() {
        let root = std::env::temp_dir().join(format!("skyward-usb-{}", std::process::id()));
        disk(&root, "platform/soc/1d84000.ufshc/host0/target0:0:0/0:0:0:0/block/sda", 61_440_000, &[("sda1", 8), ("sda2", 2_048)]);
        disk(&root, "virtual/block/loop0", 2_048, &[]);
        let usb = "platform/soc/a600000.ssusb/a600000.dwc3/xhci-hcd.0.auto/usb1";
        disk(&root, &format!("{usb}/1-1/1-1:1.0/host1/target1:0:0/1:0:0:0/block/sdg"), 60_437_492, &[("sdg2", 60_000_000), ("sdg1", 409_600)]);
        disk(&root, &format!("{usb}/1-2/1-2:1.0/host2/target2:0:0/2:0:0:0/block/sdh"), 0, &[]);
        let found = drives_in(&root.join("block")).unwrap();
        let sdg = Drive { disk: "sdg".into(), parts: vec![("sdg1".into(), 409_600 * 512), ("sdg2".into(), 60_000_000 * 512)] };
        assert_eq!(found, [sdg], "the phone's storage, a loop device and an empty card reader are left out");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_largest_partition_is_tried_first_and_a_disk_without_any_whole() {
        let mac = Drive { disk: "sdg".into(), parts: vec![("sdg1".into(), 209_715_200), ("sdg2".into(), 63_000_000_000)] };
        assert_eq!(mac.candidates(), ["sdg2", "sdg1"], "the data before the EFI partition in front of it");
        let floppy = Drive { disk: "sdh".into(), parts: Vec::new() };
        assert_eq!(floppy.candidates(), ["sdh"]);
    }

    fn boot(oem: &[u8; 8]) -> Vec<u8> {
        let mut boot = vec![0; 512];
        boot[3..11].copy_from_slice(oem);
        boot[510..512].copy_from_slice(&[0x55, 0xaa]);
        boot
    }

    #[test]
    fn ntfs_data_uses_the_reader_and_never_falls_back_to_ventoy_boot_files() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("sdg1"), boot(b"NTFS    ")).unwrap();
        let mut efi = boot(b"MSDOS5.0");
        efi[43..54].copy_from_slice(b"VTOYEFI    ");
        efi[54..62].copy_from_slice(b"FAT16   ");
        fs::write(root.path().join("sdg2"), &efi).unwrap();
        let drive = Drive { disk: "sdg".into(), parts: vec![("sdg1".into(), 500_000_000), ("sdg2".into(), 32_000_000)] };
        let mut attempted = Vec::new();
        let shown = mount_using(&drive, root.path(), |device, fs, _| {
            attempted.push((device.file_name().unwrap().to_string_lossy().to_string(), fs));
            bail!("kernel driver unavailable")
        }, |device| {
            assert!(device.ends_with("sdg1"));
            Ok(())
        }).unwrap();
        assert_eq!(shown, ("sdg1".into(), "ntfs-3g"));
        assert_eq!(attempted, [("sdg1".into(), "ntfs3"), ("sdg1".into(), "ntfs")]);

        let refused = mount_using(&drive, root.path(), |device, _, _| {
            assert!(!device.ends_with("sdg2"), "boot partition must not be mounted when data is unreadable");
            bail!("kernel driver unavailable")
        }, |_| bail!("NTFS reader missing")).unwrap_err().to_string();
        assert!(refused.contains("NTFS reader missing"));
        assert!(refused.contains("skipped Ventoy boot partition"));
        assert_eq!(fs::read(root.path().join("sdg2")).unwrap(), efi, "probing does not modify the drive");
    }

    #[test]
    fn ordinary_fat_partitions_and_unpartitioned_drives_still_mount() {
        let root = tempfile::tempdir().unwrap();
        let mut fat = boot(b"MSDOS5.0");
        fat[43..54].copy_from_slice(b"MY FILES   ");
        fat[54..62].copy_from_slice(b"FAT16   ");
        fs::write(root.path().join("sdg1"), &fat).unwrap();
        fs::write(root.path().join("sdh"), &fat).unwrap();
        for (drive, name) in [
            (Drive { disk: "sdg".into(), parts: vec![("sdg1".into(), 1_000_000)] }, "sdg1"),
            (Drive { disk: "sdh".into(), parts: vec![] }, "sdh"),
        ] {
            let shown = mount_using(&drive, root.path(), |_, fs, _| {
                assert_eq!(fs, "vfat");
                Ok(())
            }, |_| panic!("FAT must not use the NTFS reader")).unwrap();
            assert_eq!(shown, (name.into(), "vfat"));
        }
        assert_eq!(volume(&[]), Volume::Other);
        fat[510] = 0;
        assert_eq!(volume(&fat), Volume::Other);
    }

    #[test]
    fn native_ntfs_support_does_not_start_a_userspace_reader() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("sdg1"), boot(b"NTFS    ")).unwrap();
        let drive = Drive { disk: "sdg".into(), parts: vec![("sdg1".into(), 1_000_000)] };
        let shown = mount_using(&drive, root.path(), |_, filesystem, _| {
            assert_eq!(filesystem, "ntfs3");
            Ok(())
        }, |_| panic!("native NTFS does not need a helper")).unwrap();
        assert_eq!(shown, ("sdg1".into(), "ntfs3"));
        let mut fat32 = boot(b"MSWIN4.1");
        fat32[71..82].copy_from_slice(b"VTOYEFI    ");
        fat32[82..90].copy_from_slice(b"FAT32   ");
        assert_eq!(volume(&fat32), Volume::VentoyEfi);
        fat32[71] = b'X';
        assert_eq!(volume(&fat32), Volume::Other);
    }
}
