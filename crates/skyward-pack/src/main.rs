use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use skyward_config::ramdisk as paths;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

mod avb;
mod recovery;

use skyward_bootimg::{BootImage, Format, Ramdisk};

/// Builds Skyward recovery images from a device's own stock images.
#[derive(Parser)]
#[command(name = "skyward-pack")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print an image's header and the files in its ramdisk.
    List { image: PathBuf },
    /// Write one ramdisk file to stdout.
    Cat { image: PathBuf, path: String },
    /// Unpack and pack an image's ramdisk unchanged, to check that a rebuilt
    /// archive keeps every entry.
    Repack { image: PathBuf, out: PathBuf },
    /// Join a kernel and another image's ramdisk into one boot image.
    Boot {
        /// Image holding the kernel (the device's boot partition).
        #[arg(long)]
        kernel: PathBuf,
        /// Image holding the ramdisk.
        #[arg(long)]
        ramdisk: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Build a Skyward recovery image on top of the device's stock recovery:
    /// its init, SELinux policy and libraries stay; adbd may be explicitly
    /// replaced with --adbd. The recovery
    /// service becomes Skyward. Modern stock keeps its partition size and
    /// gets an unsigned AVB footer; legacy stock is rebuilt without a footer.
    Recovery {
        /// The device's stock recovery image: the whole partition, as the
        /// phone or a full update package holds it. Modern images need their
        /// AVB footer; legacy v0-v2 images may have no footer.
        #[arg(long)]
        stock: PathBuf,
        /// Actual recovery partition capacity in bytes, when known from
        /// the device. Checks that legacy output fits; never inferred from
        /// the size of a signed input file.
        #[arg(long)]
        partition_size: Option<usize>,
        /// Skyward built for the device (aarch64-linux-android).
        #[arg(long)]
        binary: PathBuf,
        /// The first program of a system installed beside the installed
        /// one, skyward-system-init built for the device and linked
        /// statically.
        #[arg(long)]
        system_init: PathBuf,
        /// NTFS reader bundle from tools/build-ntfs3g.sh (ntfs-3g,
        /// COPYING and source.tar.gz). Allows NTFS USB drives on kernels
        /// without an NTFS driver; includes its licence and source.
        #[arg(long)]
        ntfs3g: Option<PathBuf>,
        /// A local static AArch64 shell to install as /system/bin/sh when
        /// stock recovery has no usable shell. No binary is downloaded.
        #[arg(long)]
        shell: Option<PathBuf>,
        /// 导入本地静态 ARM64 adbd，替换 stock init 的现有服务程序。
        /// 免密钥需要从可信源码构建的 no-auth 实现；ELF 校验不能证明其认证行为。
        #[arg(long)]
        adbd: Option<PathBuf>,
        /// Trust this computer's adbkey.pub in recovery, including stock
        /// adbd builds that always require authentication.
        #[arg(long)]
        adb_key: Option<PathBuf>,
        /// The device configuration, devices/<codename>.toml.
        #[arg(long)]
        config: PathBuf,
        /// The UI typeface.
        #[arg(long)]
        font: PathBuf,
        /// The terminal's monospaced typeface.
        #[arg(long)]
        mono: PathBuf,
        /// The monospaced typeface's licence, which goes with it.
        #[arg(long)]
        mono_license: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Print the stock build a device's recovery is made on, as lines of
    /// name=value: its version, where its full update package is, and the
    /// SHA-256 of the recovery partition in that package. Given somewhere
    /// to write it, also take that partition out of the package on its
    /// server, reading only what the partition takes, and write it there
    /// once its SHA-256 is that one.
    Stock {
        /// The device configuration, devices/<codename>.toml.
        #[arg(long)]
        config: PathBuf,
        /// Where to write the stock recovery image.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::List { image } => list(&image),
        Cmd::Cat { image, path } => {
            let rd = Ramdisk::decode(&BootImage::read(&image)?.ramdisk)?;
            let e = rd.find(&path).with_context(|| format!("{path} is not in the ramdisk"))?;
            std::io::stdout().write_all(&e.data)?;
            Ok(())
        }
        Cmd::Repack { image, out } => {
            let mut img = BootImage::read(&image)?;
            let format = Format::of(&img.ramdisk)?;
            img.ramdisk = Ramdisk::decode(&img.ramdisk)?.encode_as(format)?;
            write(&out, &img, &img.to_bytes()?)
        }
        Cmd::Boot { kernel, ramdisk, out } => {
            let mut img = BootImage::read(&ramdisk)?;
            img.kernel = BootImage::read(&kernel)?.kernel;
            write(&out, &img, &img.to_bytes()?)
        }
        Cmd::Recovery { stock, partition_size: capacity, binary, system_init, ntfs3g, shell, adbd, adb_key, config, font, mono, mono_license, out } => {
            let mut img = BootImage::read(&stock)?;
            let size = match partition_size(&stock) {
                Ok(size) => Some(size),
                Err(_) if matches!(&img.header, abootimg_oxide::Header::V0(_)) => None,
                Err(e) => return Err(e),
            };
            if let (Some(footer_size), Some(capacity)) = (size, capacity) {
                ensure!(footer_size == capacity, "stock footer size {footer_size} differs from the supplied capacity {capacity}");
            }
            let format = Format::of(&img.ramdisk)?;
            let mut rd = Ramdisk::decode(&img.ramdisk)?;
            // A stock recovery ramdisk has no third-party tree; refuse to build
            // on top of another custom recovery.
            if let Some(e) = rd.entries.iter().find(|e| e.name == "FFiles" || e.name.starts_with("twres")) {
                anyhow::bail!("{} holds {}, which is not a stock recovery", stock.display(), e.name);
            }
            let read = |p: &PathBuf| std::fs::read(p).with_context(|| format!("read {}", p.display()));
            let cfg = skyward_config::DeviceConfig::load(&config)?;
            if let Some(version) = &cfg.recovery.legacy_version {
                let abootimg_oxide::Header::V0(header) = &mut img.header else {
                    anyhow::bail!("legacy_version requires a v0-v2 recovery image");
                };
                header.osversionpatch = abootimg_oxide::OsVersionPatch(version.encoded()?);
            }
            if cfg.recovery.ramdisk_libraries_first || cfg.recovery.ramdisk_libraries_only {
                let path = "system/etc/ld.config.txt";
                let entry = rd.find(path).context("ramdisk_libraries_first requires system/etc/ld.config.txt")?;
                let text = std::str::from_utf8(&entry.data).context("ld.config.txt is not text")?;
                let fixed = if cfg.recovery.ramdisk_libraries_only {
                    ramdisk_libraries_only(text)?
                } else {
                    ramdisk_libraries_first(text)?
                };
                rd.replace(path, fixed.into_bytes())?;
            }
            recovery::install(&mut rd, read(&binary)?)?;
            recovery::install_shell(&mut rd, shell.map(|path| read(&path)).transpose()?)?;
            if let Some(path) = adbd {
                let data = read(&path)?;
                let digest = Sha256::digest(&data);
                let target = recovery::install_adbd(&mut rd, data)?;
                println!("adbd: {} -> /{target}, SHA-256 {digest:x}；认证行为仍需源码核验与真机验证", path.display());
            }
            if let Some(key) = adb_key {
                recovery::trust_adb_key(&mut rd, &read(&key)?)?;
            }
            rd.add(paths::SYSTEM_INIT, 0o755, read(&system_init)?)?;
            if let Some(bundle) = ntfs3g {
                rd.add("skyward/ntfs-3g", 0o755, read(&bundle.join("ntfs-3g"))?)?;
                rd.add("skyward/ntfs3g-COPYING", 0o644, read(&bundle.join("COPYING"))?)?;
                rd.add("skyward/ntfs3g-source.tar.gz", 0o644, read(&bundle.join("source.tar.gz"))?)?;
            }
            rd.add(paths::CONFIG, 0o644, read(&config)?)?;
            if !cfg.fstab.is_empty() {
                let text: String = cfg.fstab.iter().map(|line| format!("{}\n", line.text())).collect();
                rd.add(paths::FSTAB, 0o644, text.into_bytes())?;
            }
            rd.add(paths::FONT, 0o644, read(&font)?)?;
            rd.add(paths::MONO, 0o644, read(&mono)?)?;
            rd.add(paths::MONO_LICENSE, 0o644, read(&mono_license)?)?;
            // A stock ramdisk may run logd without any scheduling profiles,
            // and logd stops as soon as it cannot find the one it asks for.
            if rd.find(paths::TASK_PROFILES).is_none() {
                rd.add(paths::TASK_PROFILES, 0o644, TASK_PROFILES.to_vec())?;
            }
            // Legacy init also scans /system/etc/init. Keep its explicitly
            // imported file at the ramdisk root so actions are parsed once.
            let mtp_rc = if cfg.device.ab { paths::MTP_RC } else { "skyward-mtp.rc" };
            rd.add(mtp_rc, 0o644, MTP_RC.to_vec())?;
            if !cfg.device.ab {
                let entry = rd.find("init.rc").context("legacy stock has no init.rc")?;
                let mut init = String::from_utf8(entry.data.clone()).context("init.rc is not text")?;
                if !init.contains(&format!("import /{mtp_rc}")) {
                    init = format!("import /{mtp_rc}\n{init}");
                    rd.replace("init.rc", init.into_bytes())?;
                }
                // Older vendor rules bind their kernel MTP function before
                // Skyward has supplied FunctionFS descriptors. Keep their
                // plain ADB setup, but let Skyward own the combined port.
                for entry in &mut rd.entries {
                    if entry.name.ends_with(".rc") && !entry.is_dir() {
                        if let Ok(text) = std::str::from_utf8(&entry.data) {
                            let changed = legacy_rc(&entry.name, text);
                            if changed != text { entry.data = changed.into_bytes(); }
                        }
                    }
                }
            }
            // Remove stock assignments before setting recovery properties:
            // legacy init may publish read-only properties as it parses them.
            // Stock user-build adbd may still enforce authentication; adb_keys
            // provides explicit host authorization in that case.
            // update_engine refuses a package built before the recovery
            // itself unless both of the last two are set. Skyward checks
            // device compatibility without build-date or security-patch-date limits.
            let stock_props = &rd
                .find(paths::PROP_DEFAULT)
                .with_context(|| format!("{} is not in the ramdisk", paths::PROP_DEFAULT))?
                .data;
            let mut props = recovery_props(stock_props);
            if let Some(version) = &cfg.recovery.legacy_version {
                props.extend_from_slice(format!("ro.build.version.release={}\nro.build.version.security_patch={}\n", version.release, version.security_patch).as_bytes());
            }
            rd.replace(paths::PROP_DEFAULT, props)?;
            img.ramdisk = rd.encode_as(format)?;
            let bytes = img.to_bytes()?;
            let bytes = match size {
                Some(size) => avb::footed(&bytes, "recovery", size, &Sha256::digest(&bytes))?,
                None => {
                    println!("legacy image: output is unsigned, without an AVB footer or guessed partition padding");
                    bytes
                }
            };
            if let Some(capacity) = capacity {
                ensure!(bytes.len() <= capacity, "rebuilt recovery is {} bytes, exceeding the {capacity}-byte partition; use a smaller font or reduce the recovery payload", bytes.len());
            }
            write(&out, &img, &bytes)
        }
        Cmd::Stock { config, out } => {
            let stock = skyward_config::DeviceConfig::load(&config)?.stock.with_context(|| format!("{} names no stock build", config.display()))?;
            println!("version={}\npackage={}\nrecovery={}", stock.version, stock.package, stock.recovery);
            match out {
                Some(out) => take_stock_recovery(&stock, &out),
                None => Ok(()),
            }
        }
    }
}

/// The partition a recovery is made on, as a full update package names it.
const RECOVERY: &str = "recovery";

/// Take the recovery partition out of the stock build's full update package
/// on its server, reading only what that partition takes of it, and write it
/// to `out` once its SHA-256 is the one the configuration names.
fn take_stock_recovery(stock: &skyward_config::Stock, out: &Path) -> Result<()> {
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| format!("make {}", parent.display()))?;
    // Taken out beside where it goes, so it is moved there whole.
    let work = tempfile::Builder::new().prefix(".stock").tempdir_in(parent).with_context(|| format!("make a folder in {}", parent.display()))?;
    let payload = unpayload::Payload::open_url(&stock.package).with_context(|| format!("open {}", stock.package))?;
    let options = unpayload::ExtractOptions { output_dir: work.path().to_path_buf(), partitions: vec![RECOVERY.into()], ..Default::default() };
    let report = |event: unpayload::Event<'_>| match event {
        unpayload::Event::Started { partitions } => {
            for (name, bytes) in partitions {
                println!("taking {name} out of the package: {bytes} bytes");
            }
        }
        unpayload::Event::Extracted(image) => println!("took {} out: {} bytes", image.partition, image.size),
        unpayload::Event::Progress { .. } => {}
    };
    let taken = payload.extract(&options, &report).with_context(|| format!("take {RECOVERY} out of {}", stock.package))?;
    let image = taken.iter().find(|i| i.partition == RECOVERY).with_context(|| format!("{} holds no {RECOVERY} partition", stock.package))?;
    let sum: String = image.sha256.as_deref().context("the package's own sum of the image was not checked")?.iter().map(|b| format!("{b:02x}")).collect();
    ensure!(sum == stock.recovery, "the {RECOVERY} partition in {} has the SHA-256 {sum}, where the configuration names {}", stock.package, stock.recovery);
    std::fs::rename(&image.path, out).with_context(|| format!("move {} to {}", image.path.display(), out.display()))?;
    println!("wrote {} ({} bytes), its SHA-256 the one the configuration names", out.display(), image.size);
    Ok(())
}

/// The size of the partition `stock` fills: all of it, as the phone and a
/// full update package hold it, which its AVB footer ends.
fn partition_size(stock: &Path) -> Result<usize> {
    let mut file = File::open(stock).with_context(|| format!("open {}", stock.display()))?;
    let size = file.metadata().with_context(|| format!("stat {}", stock.display()))?.len();
    let mut magic = [0u8; 4];
    if size >= 64 {
        file.seek(SeekFrom::Start(size - 64)).with_context(|| format!("seek in {}", stock.display()))?;
        file.read_exact(&mut magic).with_context(|| format!("read {}", stock.display()))?;
    }
    ensure!(
        &magic == b"AVBf",
        "{} does not end in an AVB footer; give the whole recovery partition, as the phone or a full update package holds it",
        stock.display()
    );
    Ok(usize::try_from(size)?)
}

/// What a recovery image says about the build it is: the version and patch
/// levels of its own ramdisk's system and vendor partition, which are older
/// than those installed beside it whenever the phone has taken an update.
/// The phone's secure services read these to tell the secure world which
/// build is running, and keys the installed system made are refused when
/// they say the wrong one, so they are left out here for the recovery to
/// fill in from the installed system once it has that mounted.
const BUILD_VERSION: [&str; 3] = ["ro.build.version.release", "ro.build.version.security_patch", "ro.vendor.build.security_patch"];

/// The scheduling profiles a recovery's programs look up by name. A
/// recovery's cgroup setup mounts no controllers, so of all a system's
/// profiles do, only the timer slack applies here.
const TASK_PROFILES: &[u8] = include_bytes!("task_profiles.json");

/// The init rules that give the USB port file transfer beside adb, which a
/// stock recovery does not offer.
const MTP_RC: &[u8] = include_bytes!("mtp.rc");

/// A property file without the lines that set any of `names`.
fn recovery_props(stock: &[u8]) -> Vec<u8> {
    let props = without(stock, &BUILD_VERSION);
    let mut props = without(&props, &[
        "ro.secure", "ro.adb.secure", "ro.debuggable", "ro.ota.allow_downgrade",
    ]);
    props.extend_from_slice(b"\nro.secure=0\nro.adb.secure=0\nro.debuggable=1\nro.ota.allow_downgrade=true\n");
    props
}

fn without(props: &[u8], names: &[&str]) -> Vec<u8> {
    let mut out = Vec::with_capacity(props.len());
    for line in props.split_inclusive(|b| *b == b'\n') {
        let key = line.split(|b| *b == b'=').next().unwrap_or_default();
        if !names.iter().any(|n| n.as_bytes() == key) {
            out.extend_from_slice(line);
        }
    }
    out
}

/// Reorder only the default search path of the named recovery namespace.
/// Keep namespace links, vendor rules and every other section untouched.
fn ramdisk_libraries_only(text: &str) -> Result<String> {
    let reordered = ramdisk_libraries_first(text)?;
    let mut recovery = false;
    Ok(reordered.split_inclusive('\n').filter(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            recovery = trimmed == "[recovery]";
        }
        !(recovery && trimmed.strip_prefix("namespace.default.search.paths")
            .is_some_and(|rest| rest.trim_start().starts_with("+=")))
    }).collect())
}

fn ramdisk_libraries_first(text: &str) -> Result<String> {
    const KEY: &str = "namespace.default.search.paths";
    const RAMDISK: &str = "/system/${LIB}";
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut recovery = false;
    let mut indices = Vec::new();
    let mut paths = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            recovery = trimmed == "[recovery]";
        }
        if !recovery { continue; }
        let Some((key, value)) = trimmed.split_once('=') else { continue };
        if key.trim_end().trim_end_matches('+').trim_end() == KEY {
            indices.push(i);
            paths.extend(value.trim().split(':').map(str::to_owned));
        }
    }
    ensure!(paths.iter().any(|p| p == RAMDISK), "recovery default namespace has no {RAMDISK} search path");
    paths.retain(|p| p != RAMDISK);
    paths.insert(0, RAMDISK.into());
    let first = indices[0];
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i == first {
            for (j, path) in paths.iter().enumerate() {
                let op = if j == 0 { "=" } else { "+=" };
                out.push_str(&format!("{KEY} {op} {path}\n"));
            }
        }
        if !indices.contains(&i) { out.push_str(line); }
    }
    Ok(out)
}

/// Remove only old combined-port binding actions. A standalone ADB-ready
/// trigger must not rebind ADB while Skyward is setting up MTP plus ADB.
fn legacy_rc(name: &str, text: &str) -> String {
    if name == skyward_config::ramdisk::MTP_RC || name == "skyward-mtp.rc" {
        text.to_string()
    } else { legacy_usb_rules(text) }
}

fn legacy_usb_rules(text: &str) -> String {
    let mut out = String::new();
    let mut dropping = false;
    for line in text.split_inclusive('\n') {
        if line.starts_with("on ") {
            dropping = line.contains("config=mtp,adb") &&
                (line.contains("sys.usb.ffs.ready") || line.contains("sys.usb.configfs=0"));
        } else if !(line.starts_with(char::is_whitespace) || line.trim().is_empty() || line.starts_with('#')) {
            dropping = false;
        }
        if !dropping {
            if line.trim_end() == "on property:sys.usb.ffs.ready=1" {
                out.push_str("on property:sys.usb.ffs.ready=1 && property:sys.usb.config=adb\n");
            } else { out.push_str(line); }
        }
    }
    out
}

/// Write `bytes`, the image `img` makes, to `out`.
fn write(out: &Path, img: &BootImage, bytes: &[u8]) -> Result<()> {
    std::fs::write(out, bytes).with_context(|| format!("write {}", out.display()))?;
    println!(
        "wrote {} ({} bytes kernel, {} bytes ramdisk, {} bytes in all)",
        out.display(),
        img.kernel.len(),
        img.ramdisk.len(),
        bytes.len()
    );
    Ok(())
}

fn list(image: &Path) -> Result<()> {
    let img = BootImage::read(image)?;
    let h = &img.header;
    println!(
        "header v{}  kernel {} bytes  ramdisk {} bytes  {:?}",
        h.header_version(),
        img.kernel.len(),
        img.ramdisk.len(),
        match h { abootimg_oxide::Header::V0(h) => &h.osversionpatch, abootimg_oxide::Header::V3(h) => &h.osversionpatch }
    );
    if img.ramdisk.is_empty() {
        return Ok(());
    }
    let rd = Ramdisk::decode(&img.ramdisk)?;
    for e in &rd.entries {
        println!("{:06o} {:>5}:{:<5} {:>10}  {}", e.mode, e.uid, e.gid, e.data.len(), e.name);
    }
    println!("{} entries", rd.entries.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_library_order_is_opt_in_and_section_local() {
        let stock = concat!(
            "dir.recovery = /system/bin\n[recovery]\n",
            "namespace.default.search.paths = /system_root/system/${LIB}\n",
            "namespace.default.search.paths += /system/${LIB}:/vendor/${LIB}\n",
            "namespace.default.links = system\n",
            "[vendor]\nnamespace.default.search.paths = /vendor/${LIB}\n"
        );
        let fixed = ramdisk_libraries_first(stock).unwrap();
        assert!(fixed.contains("[recovery]\nnamespace.default.search.paths = /system/${LIB}\nnamespace.default.search.paths += /system_root/system/${LIB}\n"));
        assert!(fixed.contains("namespace.default.search.paths += /vendor/${LIB}\nnamespace.default.links = system\n"));
        assert!(fixed.ends_with("[vendor]\nnamespace.default.search.paths = /vendor/${LIB}\n"));
        assert_eq!(ramdisk_libraries_first(&fixed).unwrap(), fixed);
        assert!(ramdisk_libraries_first("[vendor]\nnamespace.default.search.paths = /system/${LIB}\n").is_err());
    }

    #[test]
    fn old_recovery_can_exclude_newer_fallback_libraries() {
        let text = "dir.recovery = /system/bin\n[recovery]\nnamespace.default.search.paths = /system_root/system/${LIB}:/system/${LIB}\nnamespace.default.search.paths += /vendor/${LIB}\nnamespace.other.search.paths += /keep\n[vendor]\nnamespace.default.search.paths += /vendor/${LIB}\n";
        let fixed = ramdisk_libraries_only(text).unwrap();
        assert_eq!(fixed, "dir.recovery = /system/bin\n[recovery]\nnamespace.default.search.paths = /system/${LIB}\nnamespace.other.search.paths += /keep\n[vendor]\nnamespace.default.search.paths += /vendor/${LIB}\n");
        assert_eq!(ramdisk_libraries_only(&fixed).unwrap(), fixed);
    }

    #[test]
    fn legacy_usb_rules_keep_adb_and_remove_the_competing_mtp_binding() {
        let stock = "on property:sys.usb.config=mtp,adb\n    start adbd\n\non property:sys.usb.ffs.ready=1 && property:sys.usb.config=mtp,adb\n    symlink /functions/mtp.gs0 /config/f1\n\non property:sys.usb.ffs.ready=1\n    write /UDC controller\n";
        let changed = legacy_usb_rules(stock);
        assert!(changed.contains("on property:sys.usb.config=mtp,adb\n    start adbd"));
        assert!(!changed.contains("mtp.gs0"));
        assert!(changed.contains("on property:sys.usb.ffs.ready=1 && property:sys.usb.config=adb"));
        assert!(!changed.contains("on property:sys.usb.ffs.ready=1\n"));
        assert_eq!(legacy_usb_rules(&changed), changed);
        let modern = "on boot\n    start adbd\n";
        assert_eq!(legacy_usb_rules(modern), modern);
    }

    #[test]
    fn legacy_packing_preserves_skywards_own_mtp_binding() {
        let text = std::str::from_utf8(MTP_RC).unwrap();
        let kept = legacy_rc(skyward_config::ramdisk::MTP_RC, text);
        assert_eq!(kept, text);
        assert_eq!(legacy_rc("skyward-mtp.rc", text), text);
        let teardown = kept.split("on property:sys.usb.config=none && property:sys.usb.configfs=1\n").nth(1).unwrap();
        for command in ["stop adbd", "setprop sys.usb.ffs.ready 0",
            "rm /config/usb_gadget/g1/configs/b.1/f1", "setprop sys.usb.state none"] {
            assert!(teardown.contains(command));
        }
        assert!(kept.contains("on property:sys.usb.config=mtp,adb\n    start adbd"));
        assert!(kept.contains("on property:sys.usb.configfs=1\n"));
        assert!(!kept.contains("on fs && property:sys.usb.configfs=1"));
        assert!(kept.contains("on property:sys.usb.config=adb\n    start adbd"));
        // Combined mode waits for both ADB and MTP descriptors. ADB-only
        // binding belongs to stock init, so it has no action in this file.
        assert!(kept.contains("on property:sys.usb.config=mtp,adb && property:sys.usb.ffs.ready=1 && property:sys.usb.ffs.mtp.ready=1 && property:sys.usb.configfs=1\n"));
        assert!(kept.contains("symlink /config/usb_gadget/g1/functions/ffs.adb"));
        assert!(kept.contains("symlink /config/usb_gadget/g1/functions/ffs.mtp"));
        assert_ne!(legacy_rc("init.stock.rc", text), text);
    }

    #[test]
    fn recovery_properties_replace_all_stock_security_assignments() {
        let stock = b"ro.secure=1\nro.adb.secure=1\nro.debuggable=0\nro.adb.secure=1\nro.ota.allow_downgrade=false\nro.product.device=wayne\n";
        let props = recovery_props(stock);
        let text = std::str::from_utf8(&props).unwrap();
        for (key, value) in [("ro.secure", "0"), ("ro.adb.secure", "0"),
            ("ro.debuggable", "1"), ("ro.ota.allow_downgrade", "true")] {
            let lines: Vec<_> = text.lines().filter(|line|
                line.split_once('=').is_some_and(|(name, _)| name == key)).collect();
            assert_eq!(lines, vec![format!("{key}={value}")]);
        }
        assert!(text.contains("ro.product.device=wayne\n"));
    }

    #[test]
    fn the_lines_that_set_a_name_are_the_only_ones_left_out() {
        let props = b"ro.build.id=TQ\nro.build.version.release=13\nro.build.version.release_or_codename=13\nro.product.name=ishtar\n\
ro.vendor.build.security_patch=2026-10-01\nro.vendor.build.security_patch_level=1\n";
        let out = without(props, &BUILD_VERSION);
        let text = String::from_utf8(out).expect("property files are text");
        assert!(!text.contains("ro.build.version.release=13"));
        assert!(!text.contains("ro.vendor.build.security_patch=2026-10-01"), "the vendor partition's patch level is left out too");
        assert!(text.contains("ro.build.version.release_or_codename=13"), "a longer name is left alone");
        assert!(text.contains("ro.vendor.build.security_patch_level=1"), "a longer name is left alone");
        assert!(text.contains("ro.build.id=TQ") && text.contains("ro.product.name=ishtar"));
    }

    #[test]
    fn the_profiles_name_all_the_recovery_asks_for() {
        let profiles: serde_json::Value = serde_json::from_slice(TASK_PROFILES).expect("the profiles are JSON");
        let names = |key: &str| -> Vec<String> {
            profiles[key].as_array().expect("a list").iter().map(|p| p["Name"].as_str().expect("a name").to_string()).collect()
        };
        let (plain, aggregate) = (names("Profiles"), names("AggregateProfiles"));
        // What libprocessgroup asks for when a program sets its scheduling
        // policy, and what the recovery's own init scripts give logd.
        let asked = [
            "SCHED_SP_DEFAULT",
            "SCHED_SP_BACKGROUND",
            "SCHED_SP_FOREGROUND",
            "SCHED_SP_TOP_APP",
            "SCHED_SP_SYSTEM",
            "SCHED_SP_RT_APP",
            "CPUSET_SP_DEFAULT",
            "CPUSET_SP_BACKGROUND",
            "CPUSET_SP_FOREGROUND",
            "CPUSET_SP_TOP_APP",
            "CPUSET_SP_SYSTEM",
            "CPUSET_SP_RESTRICTED",
            "BlkIOBackground",
            "BlkIOForeground",
            "ServiceCapacityLow",
        ];
        for name in asked {
            assert!(plain.iter().chain(&aggregate).any(|n| n == name), "{name} is missing");
        }
        for a in profiles["AggregateProfiles"].as_array().expect("a list") {
            for part in a["Profiles"].as_array().expect("a list") {
                let part = part.as_str().expect("a name");
                assert!(plain.iter().any(|n| n == part), "{} is made of {part}, which is not a profile", a["Name"]);
            }
        }
    }

    #[test]
    fn the_partition_is_as_large_as_a_stock_image_ending_in_its_footer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.img");
        let mut whole = vec![0u8; 8192];
        whole[8192 - 64..8192 - 60].copy_from_slice(b"AVBf");
        std::fs::write(&path, &whole).unwrap();
        assert_eq!(partition_size(&path).unwrap(), 8192);
        std::fs::write(&path, &whole[..8000]).unwrap();
        assert!(partition_size(&path).is_err(), "an image cut short of its partition's end");
        std::fs::write(&path, b"AVBf").unwrap();
        assert!(partition_size(&path).is_err(), "too short to end in a footer");
    }

    #[test]
    fn a_file_without_a_last_newline_keeps_its_last_line() {
        let out = without(b"a=1\nro.build.version.release=13", &BUILD_VERSION);
        assert_eq!(out, b"a=1\n");
        let kept = without(b"a=1\nb=2", &BUILD_VERSION);
        assert_eq!(kept, b"a=1\nb=2");
    }
}
