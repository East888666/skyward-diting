//! Refresh an init-owned service directory after publishing descriptions.
//! A configured directory starts later and reads the new files on its first
//! start; it must not be queried or restarted before it exists.

use anyhow::Result;

/// The query asks whether a needed declaration is missing from the running
/// directory. Restarting also restores the services registered with it.
pub fn refresh(
    init_owned: bool,
    rewritten: bool,
    unknown: impl FnOnce() -> Result<bool>,
    restart: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if !init_owned {
        return Ok(());
    }
    if unknown()? || rewritten {
        restart()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn configured_directory_can_start_with_no_binder_peer() {
        for rewritten in [false, true] {
            refresh(
                false,
                rewritten,
                || anyhow::bail!("the other side is gone"),
                || anyhow::bail!("init has no servicemanager service"),
            )
            .unwrap();
        }
    }

    #[test]
    fn init_directory_keeps_registered_services_when_descriptions_are_current() {
        refresh(true, false, || Ok(false), || panic!("must not restart")).unwrap();
    }

    #[test]
    fn init_directory_restarts_for_missing_or_rewritten_descriptions() {
        for (unknown, rewritten) in [(true, false), (false, true), (true, true)] {
            let restarted = Cell::new(0);
            refresh(true, rewritten, || Ok(unknown), || {
                restarted.set(restarted.get() + 1);
                Ok(())
            })
            .unwrap();
            assert_eq!(restarted.get(), 1);
        }
    }

    #[test]
    fn init_directory_query_failure_remains_visible() {
        let result = refresh(
            true,
            false,
            || anyhow::bail!("directory query failed"),
            || panic!("must not restart after a failed query"),
        );
        assert_eq!(result.unwrap_err().to_string(), "directory query failed");
    }

    #[test]
    fn init_directory_restart_failure_remains_visible() {
        let result = refresh(true, false, || Ok(true), || anyhow::bail!("restart failed"));
        assert_eq!(result.unwrap_err().to_string(), "restart failed");
    }
}
