//! Explicitly selected compatibility library for legacy RPMB clients.

use anyhow::Result;
use skyward_bootimg::Ramdisk;

const BINARY: &[u8] = include_bytes!("../assets/legacy-rpmb/liblegacy-rpmb-compat.so");
const SOURCE: &[u8] = include_bytes!("../assets/legacy-rpmb/rpmb-compat.c");
const HEADER: &[u8] = include_bytes!("../assets/legacy-rpmb/rpmb-compat.h");
const README: &[u8] = include_bytes!("../assets/legacy-rpmb/README.md");

pub fn install_selected(rd: &mut Ramdisk, enabled: bool) -> Result<()> {
    if enabled {
        rd.add("skyward/liblegacy-rpmb-compat.so", 0o644, BINARY.to_vec())?;
        rd.add("skyward/rpmb-compat.c", 0o644, SOURCE.to_vec())?;
        rd.add("skyward/rpmb-compat.h", 0o644, HEADER.to_vec())?;
        rd.add("skyward/rpmb-compat-README.md", 0o644, README.to_vec())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn only_selected_devices_receive_the_verified_arm64_adapter() {
        let mut rd = Ramdisk { entries: Vec::new() };
        install_selected(&mut rd, false).unwrap();
        assert!(rd.entries.is_empty());
        install_selected(&mut rd, true).unwrap();
        let library = rd.find("skyward/liblegacy-rpmb-compat.so").unwrap();
        assert_eq!(library.mode, 0o100644);
        assert_eq!(&library.data[..7], b"\x7fELF\x02\x01\x01");
        assert_eq!(&library.data[18..20], &183u16.to_le_bytes());
        assert_eq!(hex::encode(Sha256::digest(&library.data)),
            "2685a381cb1878d349cc96f35853d1b45ee5aad1948b5ae12f6705663640a531");
        assert_eq!(rd.find("skyward/rpmb-compat.c").unwrap().data, SOURCE);
    }
}
