//! The CPUs, run as the phone's own system runs them once it has started.
//! A recovery's init leaves them as the kernel starts them: every core at
//! its top frequency, and on Qualcomm phones none powered down while idle.

use anyhow::{Context, Result};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

/// Where the kernel keeps the CPUs' frequency policies, one per cluster.
const CPUFREQ: &str = "/sys/devices/system/cpu/cpufreq";

/// The governors that set a cluster's frequency by its load, the one to
/// take first: Qualcomm's, which the phones' own boot scripts set, then the
/// kernel's own.
const GOVERNORS: [&str; 2] = ["walt", "schedutil"];

/// Qualcomm's CPU idle driver keeps idle cores from powering down until it
/// is told to let them, as the phones' own boot scripts tell it.
const SLEEP_DISABLED: &str = "/sys/devices/system/cpu/qcom_lpm/parameters/sleep_disabled";

/// Have each cluster's frequency follow its load, and let idle cores power
/// down where Qualcomm's idle driver keeps them up: what was done, for the
/// log.
pub fn run_as_system() -> Result<Vec<String>> {
    let mut policies: Vec<_> = fs::read_dir(CPUFREQ)
        .with_context(|| format!("list {CPUFREQ}"))?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<_>>()
        .with_context(|| format!("list {CPUFREQ}"))?;
    policies.retain(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("policy")));
    policies.sort();
    let mut done = Vec::new();
    for policy in &policies {
        let name = policy.file_name().expect("a policy has a name").to_string_lossy().into_owned();
        let available = fs::read_to_string(policy.join("scaling_available_governors")).with_context(|| format!("read {name}'s governors"))?;
        match governor(&available) {
            Some(g) => {
                fs::write(policy.join("scaling_governor"), g).with_context(|| format!("set {name}'s governor to {g}"))?;
                done.push(format!("{name} {g}"));
            }
            None => done.push(format!("{name} offers none of {GOVERNORS:?}, only {}", available.trim())),
        }
    }
    if Path::new(SLEEP_DISABLED).exists() {
        fs::write(SLEEP_DISABLED, "N").with_context(|| format!("write {SLEEP_DISABLED}"))?;
        done.push("idle cores power down".into());
    }
    Ok(done)
}

/// Ask a matching Qualcomm WALT scheduler to prefer its highest-capacity
/// cluster for this thread. The device must explicitly opt in: its per-task
/// ABI calls mode 2 TASK_BOOST_ON_MAX, an advisory preference, not the
/// STRICT_MAX mode 3. All CPUs remain allowed and thermal limits still apply.
/// New worker threads have their own scheduler state; no global boost or
/// frequency control is changed. An absent interface is a supported no-op.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn prefer_big_for_ui() -> Result<bool> {
    // Some Qualcomm kernels expose this vendor entry under /proc/<tid>
    // but omit it from the /proc/<pid>/task/<tid> view (thread-self).
    // SAFETY: gettid takes no pointers and returns this calling thread's ID.
    let tid = unsafe { libc::syscall(libc::SYS_gettid) };
    prefer_big_at(&Path::new("/proc").join(tid.to_string()).join("sched_boost"))
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn prefer_big_for_ui() -> Result<bool> {
    Ok(false)
}

fn prefer_big_at(control: &Path) -> Result<bool> {
    let previous = match fs::read_to_string(control) {
        Ok(value) => value,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("read {}", control.display())),
    };
    match previous.trim() {
        "0" => {}
        "2" | "3" => return Ok(true),
        // Preserve a preference installed by init or another owner.
        "1" => return Ok(false),
        other => anyhow::bail!("unexpected UI scheduler boost {other:?} at {}", control.display()),
    }
    // Never create a regular file where a kernel interface is absent.
    fs::OpenOptions::new().write(true).open(control)
        .and_then(|mut file| file.write_all(b"2"))
        .with_context(|| format!("set the UI scheduler preference at {}", control.display()))?;
    Ok(true)
}

/// The governor to run a cluster with, of the ones `available` lists.
fn governor(available: &str) -> Option<&'static str> {
    GOVERNORS.into_iter().find(|g| available.split_whitespace().any(|a| a == *g))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_boost_preserves_existing_hints_and_never_creates_a_missing_interface() {
        let dir = std::env::temp_dir().join(format!("skyward-ui-boost-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let control = dir.join("sched_boost");
        assert!(!prefer_big_at(&control).unwrap());
        assert!(!control.exists());
        for (value, active) in [("1\n", false), ("2\n", true), ("3\n", true)] {
            fs::write(&control, value).unwrap();
            assert_eq!(prefer_big_at(&control).unwrap(), active);
            assert_eq!(fs::read_to_string(&control).unwrap(), value);
        }
        fs::write(&control, "unsupported").unwrap();
        assert!(prefer_big_at(&control).is_err());
        assert_eq!(fs::read_to_string(&control).unwrap(), "unsupported");
        fs::write(&control, "0\n").unwrap();
        assert!(prefer_big_at(&control).unwrap());
        assert_eq!(fs::read_to_string(&control).unwrap().trim(), "2");
        fs::remove_file(control).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn a_cluster_runs_qualcomm_s_governor_where_it_has_it_and_the_kernel_s_otherwise() {
        assert_eq!(governor("walt conservative powersave performance schedutil \n"), Some("walt"));
        assert_eq!(governor("conservative ondemand userspace powersave performance schedutil"), Some("schedutil"));
        assert_eq!(governor("performance"), None);
        assert_eq!(governor("walty schedutilx"), None, "a name is matched whole");
    }
}
