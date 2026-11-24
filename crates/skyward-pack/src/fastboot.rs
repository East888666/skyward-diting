//! Opt-out userspace fastboot trimming. Shared init/ADB libraries stay.

use anyhow::{Context, Result};
use skyward_bootimg::Ramdisk;
use std::collections::BTreeSet;
use std::path::Path;

pub fn configure(rd: &mut Ramdisk, enabled: bool) -> Result<()> {
    if enabled {
        return Ok(());
    }
    let mut services = BTreeSet::from(["fastbootd".to_string(), "fastboot".to_string()]);
    let mut executables = BTreeSet::from([
        "system/bin/fastbootd".to_string(),
        "sbin/fastbootd".to_string(),
    ]);
    for entry in &rd.entries {
        if entry.name.ends_with(".rc") && !entry.is_dir() {
            let text = std::str::from_utf8(&entry.data)
                .with_context(|| format!("read {} while disabling fastbootd", entry.name))?;
            for (_, line) in logical_lines(text) {
                let words: Vec<_> = words(&line).collect();
                if words.first() == Some(&"service")
                    && (words.get(1) == Some(&"fastbootd")
                        || words.get(2).is_some_and(|p| {
                            Path::new(p).file_name().is_some_and(|p| p == "fastbootd")
                        }))
                {
                    if let Some(name) = words.get(1) {
                        services.insert((*name).to_owned());
                    }
                    // Never remove a shared recovery multicall executable.
                    if let Some(path) = words
                        .get(2)
                        .filter(|p| Path::new(p).file_name().is_some_and(|p| p == "fastbootd"))
                    {
                        executables.insert(path.trim_start_matches('/').to_owned());
                    }
                }
            }
        }
    }
    for entry in &mut rd.entries {
        if entry.name.ends_with(".rc") && !entry.is_dir() {
            let text = std::str::from_utf8(&entry.data)?;
            entry.data = without_fastboot(text, &services).into_bytes();
        }
    }
    for path in executables {
        rd.drop_at(&path);
    }
    rd.drop_at("res/images/fastbootd.png");
    Ok(())
}

fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split('#')
        .next()
        .unwrap_or_default()
        .split_whitespace()
}

/// Keep the original bytes for retained lines, while interpreting continued
/// init statements as one header/command.
fn logical_lines(text: &str) -> Vec<(String, String)> {
    let mut lines = Vec::new();
    let (mut raw, mut logical) = (String::new(), String::new());
    for line in text.split_inclusive('\n') {
        raw.push_str(line);
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(part) = line.strip_suffix('\\') {
            logical.push_str(part);
            logical.push(' ');
        } else {
            logical.push_str(line);
            lines.push((std::mem::take(&mut raw), std::mem::take(&mut logical)));
        }
    }
    if !raw.is_empty() {
        lines.push((raw, logical));
    }
    lines
}

fn fastboot_mode(word: &str) -> bool {
    ["property:sys.usb.config=", "property:sys.usb.state="]
        .iter()
        .filter_map(|prefix| word.strip_prefix(prefix))
        .any(|value| value.split(',').any(|function| function == "fastboot"))
}

fn without_fastboot(text: &str, services: &BTreeSet<String>) -> String {
    let mut out = String::new();
    let mut dropping = false;
    for (raw, line) in logical_lines(text) {
        let fields: Vec<_> = words(&line).collect();
        let Some(&command) = fields.first() else {
            if !dropping {
                out.push_str(&raw);
            }
            continue;
        };
        if !line.starts_with(char::is_whitespace) {
            dropping = (command == "service"
                && fields.get(1).is_some_and(|name| services.contains(*name)))
                || (command == "on" && fields.iter().any(|word| fastboot_mode(word)));
        }
        if dropping {
            continue;
        }
        if matches!(command, "start" | "stop" | "restart" | "enable")
            && fields.get(1).is_some_and(|name| services.contains(*name))
        {
            continue;
        }
        // The legacy gadget can advertise several FunctionFS aliases in
        // one command. Remove only fastboot and keep ADB's alias.
        if command == "write"
            && fields
                .get(1)
                .is_some_and(|path| path.ends_with("/f_ffs/aliases"))
        {
            if let Some(value) = fields
                .get(2)
                .filter(|v| v.split(',').any(|v| v == "fastboot"))
            {
                let kept: Vec<_> = value.split(',').filter(|v| *v != "fastboot").collect();
                if !kept.is_empty() {
                    let indent = &line[..line.len() - line.trim_start().len()];
                    out.push_str(&format!(
                        "{indent}write {} {}
",
                        fields[1],
                        kept.join(",")
                    ));
                }
                continue;
            }
        }
        if fields.iter().any(|word| {
            word.contains("/usb-ffs/fastboot") || word.contains("/functions/ffs.fastboot")
        }) {
            continue;
        }
        out.push_str(&raw);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Ramdisk {
        let mut rd = Ramdisk {
            entries: Vec::new(),
        };
        for name in [
            "system/bin/fastbootd",
            "res/images/fastbootd.png",
            "system/lib64/liblp.so",
            "system/bin/init",
            "system/bin/adbd",
            "system/bin/recovery",
        ] {
            rd.add(name, 0o755, name.as_bytes().to_vec()).unwrap();
        }
        rd.add(
            "init.rc",
            0o644,
            b"service fastbootd /system/bin/fastbootd
    disabled
    group system

service adbd /system/bin/adbd
    disabled

on fs
    mkdir /dev/usb-ffs/fastboot
    mount functionfs fastboot /dev/usb-ffs/fastboot
    mkdir /dev/usb-ffs/adb
    mount functionfs adb /dev/usb-ffs/adb
    write /sys/class/android_usb/android0/f_ffs/aliases adb,fastboot

on property:sys.usb.config=fastboot && \
    property:sys.usb.ffs.ready=1
    start fastbootd
    write /config/usb_gadget/g1/UDC controller

on property:sys.usb.config=none
    stop adbd
    stop fastbootd
    setprop sys.usb.state none

on property:sys.usb.config=adb
    start adbd
"
            .to_vec(),
        )
        .unwrap();
        rd
    }

    #[test]
    fn default_enabled_keeps_every_entry_and_byte() {
        let mut rd = fixture();
        let before: Vec<_> = rd
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.mode, e.data.clone()))
            .collect();
        configure(&mut rd, true).unwrap();
        let after: Vec<_> = rd
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.mode, e.data.clone()))
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn disabled_removes_only_dedicated_resources_and_keeps_adb_init_and_shared_libraries() {
        let mut rd = fixture();
        configure(&mut rd, false).unwrap();
        assert!(rd.find("system/bin/fastbootd").is_none());
        assert!(rd.find("res/images/fastbootd.png").is_none());
        for name in [
            "system/lib64/liblp.so",
            "system/bin/init",
            "system/bin/adbd",
            "system/bin/recovery",
        ] {
            assert_eq!(rd.find(name).unwrap().data, name.as_bytes());
        }
        let rc = std::str::from_utf8(&rd.find("init.rc").unwrap().data).unwrap();
        assert!(!rc.contains("fastboot"), "{rc}");
        assert!(rc.contains(
            "service adbd /system/bin/adbd
    disabled"
        ));
        assert!(rc.contains("mount functionfs adb /dev/usb-ffs/adb"));
        assert!(rc.contains(
            "/f_ffs/aliases adb
"
        ));
        assert!(rc.contains(
            "stop adbd
    setprop sys.usb.state none"
        ));
        assert!(rc.contains(
            "on property:sys.usb.config=adb
    start adbd"
        ));
        let before = rc.to_owned();
        configure(&mut rd, false).unwrap();
        assert_eq!(before.as_bytes(), rd.find("init.rc").unwrap().data);
    }

    #[test]
    fn removing_a_fastbootd_link_preserves_its_multicall_target() {
        let mut rd = fixture();
        rd.drop_at("system/bin/fastbootd");
        rd.link("system/bin/fastbootd", "recovery").unwrap();
        configure(&mut rd, false).unwrap();
        assert!(rd.find("system/bin/fastbootd").is_none());
        assert!(rd.find("system/bin/recovery").is_some());
    }
}
