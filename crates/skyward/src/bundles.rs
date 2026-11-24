//! Locate both packaged APEX files and flattened APEX directories.
//! Keep the existing packaged search order when both forms are present.

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    Packaged(PathBuf),
    Flattened(PathBuf),
}

pub fn find(directories: &[&Path], name: &str) -> Option<Source> {
    find_exact(directories, name).or_else(|| {
        // Android 10 installs this variant under its build name while
        // binaries still refer to the canonical com.android.runtime path.
        (name == "com.android.runtime").then(|| find_exact(directories, "com.android.runtime.release")).flatten()
    })
}

fn find_exact(directories: &[&Path], name: &str) -> Option<Source> {
    if let Some(file) = directories.iter().map(|dir| dir.join(format!("{name}.apex"))).find(|p| p.is_file()) {
        return Some(Source::Packaged(file));
    }
    directories.iter().map(|dir| dir.join(name)).find(|dir| {
        dir.is_dir() && ["apex_manifest.pb", "apex_manifest.json"].iter().any(|name| dir.join(name).is_file())
    }).map(Source::Flattened)
}

/// The APEX module named by a configured canonical loader/library path.
pub fn named_by(path: &Path) -> Option<&str> {
    let mut parts = path.strip_prefix("/apex").ok()?.components();
    match parts.next()? {
        std::path::Component::Normal(name) => name.to_str(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn configured_paths_name_only_canonical_apex_modules() {
        assert_eq!(named_by(Path::new("/apex/com.android.runtime/bin/linker64")), Some("com.android.runtime"));
        assert_eq!(named_by(Path::new("/apex/com.android.vndk.current/lib64")), Some("com.android.vndk.current"));
        for path in ["/apex", "/apex/../vendor/lib64", "/apex-other/mod/lib64", "/vendor/lib64"] {
            assert_eq!(named_by(Path::new(path)), None);
        }
    }

    #[test]
    fn flattened_bundles_need_a_manifest() {
        let root = tempfile::tempdir().unwrap();
        let at = root.path().join("com.android.runtime");
        fs::create_dir(&at).unwrap();
        assert_eq!(find(&[root.path()], "com.android.runtime"), None);
        for manifest in ["apex_manifest.pb", "apex_manifest.json"] {
            fs::write(at.join(manifest), b"manifest").unwrap();
            assert_eq!(find(&[root.path()], "com.android.runtime"), Some(Source::Flattened(at.clone())));
            fs::remove_file(at.join(manifest)).unwrap();
        }
    }

    #[test]
    fn packaged_search_order_is_preserved_before_flattened_fallback() {
        let root = tempfile::tempdir().unwrap();
        let (first, second) = (root.path().join("system_ext"), root.path().join("system"));
        let flat = first.join("com.android.runtime");
        fs::create_dir_all(&flat).unwrap();
        fs::write(flat.join("apex_manifest.pb"), b"manifest").unwrap();
        fs::create_dir(&second).unwrap();
        let later = second.join("com.android.runtime.apex");
        fs::write(&later, b"packaged").unwrap();
        assert_eq!(find(&[&first, &second], "com.android.runtime"), Some(Source::Packaged(later)));
        let earlier = first.join("com.android.runtime.apex");
        fs::write(&earlier, b"packaged").unwrap();
        assert_eq!(find(&[&first, &second], "com.android.runtime"), Some(Source::Packaged(earlier)));
    }

    #[test]
    fn android10_release_variant_is_only_a_fallback_for_runtime() {
        let root = tempfile::tempdir().unwrap();
        let release = root.path().join("com.android.runtime.release");
        fs::create_dir(&release).unwrap();
        assert_eq!(find(&[root.path()], "com.android.runtime"), None);
        fs::write(release.join("apex_manifest.json"), br#"{"name":"com.android.runtime"}"#).unwrap();
        assert_eq!(find(&[root.path()], "com.android.runtime"), Some(Source::Flattened(release.clone())));
        assert_eq!(find(&[root.path()], "com.android.vndk.current"), None);
        let canonical = root.path().join("com.android.runtime");
        fs::create_dir(&canonical).unwrap();
        fs::write(canonical.join("apex_manifest.pb"), b"manifest").unwrap();
        assert_eq!(find(&[root.path()], "com.android.runtime"), Some(Source::Flattened(canonical)));
        fs::remove_dir_all(root.path().join("com.android.runtime")).unwrap();
        let packaged = root.path().join("com.android.runtime.release.apex");
        fs::write(&packaged, b"packaged").unwrap();
        assert_eq!(find(&[root.path()], "com.android.runtime"), Some(Source::Packaged(packaged)));
    }

    #[test]
    fn missing_bundle_does_not_select_a_different_bundle() {
        let root = tempfile::tempdir().unwrap();
        let flat = root.path().join("com.android.i18n");
        fs::create_dir(&flat).unwrap();
        fs::write(flat.join("apex_manifest.pb"), b"manifest").unwrap();
        assert_eq!(find(&[root.path()], "com.android.runtime"), None);
    }
}
