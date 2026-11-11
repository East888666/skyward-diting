//! Read-only probe for the Keymaster 3 service used by older vendor images.
#[cfg(target_os = "android")]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use skyward_binder::{
        driver::Binder,
        hidl::{self, Answer, Message},
    };
    let binder = Binder::hardware()?;
    let interface = "android.hardware.keymaster@3.0::IKeymasterDevice";
    let device =
        hidl::find(&binder, interface, "default")?.context("Keymaster 3 is not registered")?;
    println!("interface: {}", hidl::describes(&device)?);
    let reply = device.transact(1, &Message::call(interface))?;
    let mut read = Answer::new(reply.bytes());
    read.status()?;
    for label in ["secure", "EC", "symmetric", "attestation", "all digests"] {
        println!("{label}: {}", read.bool()?);
    }
    for label in ["name", "author"] {
        let frame = read.pointed()?;
        anyhow::ensure!(frame.len() == 16, "invalid string frame");
        let length = u32::from_le_bytes(frame[8..12].try_into()?) as usize;
        let bytes = read.pointed()?;
        anyhow::ensure!(length < bytes.len(), "invalid string length");
        println!("{label}: {}", String::from_utf8_lossy(&bytes[..length]));
    }
    if let Some(path) = std::env::args_os().nth(1) {
        use skyward_crypto::{keymaster3::Keymaster3, keystore::{StoredKey, Unwrap}};
        let dir = std::path::Path::new(&path);
        let blob = std::fs::read(dir.join("keymaster_key_blob"))?;
        let app_id = skyward_crypto::keystore::hash_with_prefix(b"Android secdiscardable SHA512", &std::fs::read(dir.join("secdiscardable"))?);
        let mut call = Message::call(interface);
        call.bytes(blob.strip_prefix(b"pKMblob\0").unwrap_or(&blob));
        call.bytes(&app_id);
        call.bytes(&[]);
        let reply = device.transact(5, &call)?;
        let mut read = Answer::new(reply.bytes());
        read.status()?;
        let code = read.i32()?;
        println!("key characteristics status: {code}");
        if code == 0 {
            let frames = read.pointed()?;
            anyhow::ensure!(frames.len() == 32, "invalid key characteristics");
            for (label, frame) in ["software", "hardware"].iter().zip(frames.as_chunks::<16>().0) {
                let count = u32::from_le_bytes(frame[8..12].try_into()?) as usize;
                let raw = read.pointed()?;
                anyhow::ensure!(raw.len() == count * 32, "invalid characteristic parameter count");
                for p in raw.as_chunks::<32>().0 {
                    let tag = u32::from_le_bytes(p[..4].try_into()?);
                    let value = u64::from_le_bytes(p[8..16].try_into()?);
                    let _ = read.pointed()?;
                    if matches!(tag, 0x300002c1 | 0x300002c2 | 0x300002ce | 0x300002cf) {
                        println!("{label} tag {tag:#x}: {value}");
                    }
                }
            }
        }
        let service = Keymaster3::open(&binder)?;
        let key = StoredKey::load(std::path::Path::new(&path))?.unwrap(Unwrap::Hardware(&service))?;
        println!("authenticated key bytes: {} (key not exported)", key.len());
    }
    Ok(())
}
#[cfg(not(target_os = "android"))]
fn main() {
    eprintln!("Run this probe on an Android recovery.");
}
