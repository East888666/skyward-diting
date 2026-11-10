//! Expose firmware before probing drivers, including files hidden by mounts.

use anyhow::{Context, Result};
use skyward_config::Firmware;
use std::fs;
use std::path::Path;

/// A nonrecursive bind reveals the partition's own files without mounts
/// covering subdirectories, such as modem mounted over vendor/firmware.
pub fn place(file: &Firmware) -> Result<()> {
    let to = Path::new("/etc/firmware").join(&file.name);
    fs::create_dir_all(to.parent().expect("a firmware filename"))
        .with_context(|| format!("make folder for {}", to.display()))?;
    let bytes = match &file.partition {
        None => fs::read(&file.path).with_context(|| format!("read {}", file.path))?,
        Some(partition) => {
            let at = Path::new("/skyward/firmware").join(partition);
            fs::create_dir_all(&at).with_context(|| format!("make {}", at.display()))?;
            let slot = crate::booted_slot()?;
            let logical = skyward_vab::mounted_partitions(slot)?
                .into_iter()
                .find(|m| m.partition == format!("{partition}{}", slot.suffix()));
            let device = logical
                .as_ref()
                .map(|m| m.device.clone())
                .unwrap_or_else(|| Path::new(skyward_block::BY_NAME).join(partition));
            let mounted = skyward_block::mount::points_of(skyward_block::devnum(&device)?)?;
            match mounted.first() {
                Some(from) => skyward_block::mount::bind(from, &at)?,
                None => {
                    skyward_block::mount::mount_ro(&device, &at)?;
                }
            }
            let read = fs::read(at.join(&file.path))
                .with_context(|| format!("read {} on {partition}", file.path));
            let unmounted = skyward_block::mount::umount(&at);
            let bytes = read?;
            unmounted?;
            bytes
        }
    };
    fs::write(&to, bytes).with_context(|| format!("write {}", to.display()))
}
