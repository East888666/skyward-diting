//! All aliases and nested mounts must leave before a partition is written.

use std::path::{Path, PathBuf};

pub fn plan(roots: &[PathBuf], mounts: &[(PathBuf, (u32, u32))]) -> Vec<PathBuf> {
    let devices: Vec<_> = mounts
        .iter()
        .filter(|(p, _)| roots.contains(p))
        .map(|(_, d)| *d)
        .collect();
    let mut points: Vec<_> = mounts
        .iter()
        .filter(|(p, d)| {
            p != Path::new("/")
                && !p.starts_with("/data")
                && !p.starts_with("/sdcard")
                && (devices.contains(d) || roots.iter().any(|r| p.starts_with(r)))
        })
        .map(|(p, _)| p.clone())
        .collect();
    points.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    points.dedup();
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_nested_mounts_and_aliases_before_their_parent() {
        let roots = ["/system_root", "/vendor", "/vendor/firmware_mnt"].map(PathBuf::from);
        let mounts = [
            ("/", (0, 1)),
            ("/system_root", (8, 1)),
            ("/vendor", (8, 2)),
            ("/vendor/firmware_mnt", (8, 3)),
            ("/firmware", (8, 3)),
            ("/mnt/vendor_alias", (8, 2)),
            ("/data", (8, 4)),
            ("/sdcard", (8, 4)),
        ]
        .map(|(p, d)| (PathBuf::from(p), d));
        let points = plan(&roots, &mounts);
        assert!(points.contains(&PathBuf::from("/firmware")));
        assert!(points.contains(&PathBuf::from("/mnt/vendor_alias")));
        assert!(
            points
                .iter()
                .position(|p| p == Path::new("/vendor/firmware_mnt"))
                .unwrap()
                < points
                    .iter()
                    .position(|p| p == Path::new("/vendor"))
                    .unwrap()
        );
        assert!(!points
            .iter()
            .any(|p| p == Path::new("/") || p.starts_with("/data") || p.starts_with("/sdcard")));
    }

    #[test]
    fn path_prefix_is_component_based() {
        let mounts = [(PathBuf::from("/vendor_other"), (8, 8))];
        assert!(plan(&[PathBuf::from("/vendor")], &mounts).is_empty());
    }
}
