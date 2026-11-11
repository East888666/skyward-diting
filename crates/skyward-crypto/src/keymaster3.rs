//! Android Keymaster 3.0 HIDL, for vendor images predating AIDL KeyMint.
//! Only authenticated AES-GCM decryption is exposed. No key generation,
//! deletion, disk upgrade, credential enrolment or property override occurs.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use anyhow::{ensure, Result};
#[cfg(target_os = "android")]
use anyhow::Context;
use skyward_binder::hidl::{Answer, Message};
use skyward_binder::status::Fault;

pub const INTERFACE: &str = "android.hardware.keymaster@3.0::IKeymasterDevice";
const BEGIN: u32 = 12;
const UPDATE: u32 = 13;
const FINISH: u32 = 14;
const ABORT: u32 = 15;
const PARAM_BYTES: usize = 32;

/// Android 12's keystore2 compatibility layer prefixes HAL blobs with
/// "pKMblob" and an origin byte. Direct HIDL calls bypass that layer.
/// Strip its hardware marker in memory only; software blobs require the
/// software KeyMint implementation and must not reach this vendor HAL.
fn hardware_blob(blob: &[u8]) -> Result<&[u8]> {
    if blob.len() >= 8 && &blob[..7] == b"pKMblob" {
        match blob[7] {
            0 => return Ok(&blob[8..]),
            1 => anyhow::bail!("software KeyMint blob cannot be opened by Keymaster 3"),
            _ => {}
        }
    }
    Ok(blob)
}

fn parameters(call: &mut Message, app_id: Option<&[u8]>, nonce: Option<&[u8]>) -> Result<()> {
    let mut values: Vec<(u32, u64, &[u8])> = Vec::new();
    if let Some(app_id) = app_id {
        values.extend([(0x20000004, 32, &[][..]), (0x20000006, 1, &[][..]), (0x300003eb, 128, &[][..]), (0x90000259, 0, app_id)]);
    }
    if let Some(nonce) = nonce { values.push((0x900003e9, 0, nonce)); }
    let mut raw = vec![0; values.len() * PARAM_BYTES];
    for (i, (tag, value, _)) in values.iter().enumerate() {
        raw[i * PARAM_BYTES..i * PARAM_BYTES + 4].copy_from_slice(&tag.to_le_bytes());
        raw[i * PARAM_BYTES + 8..i * PARAM_BYTES + 16].copy_from_slice(&value.to_le_bytes());
    }
    let children: Vec<_> = values.iter().enumerate().map(|(i, (_, _, bytes))| (i * PARAM_BYTES + 16, *bytes)).collect();
    call.structures(&raw, PARAM_BYTES, &children)
}

fn upgrade_parameters(call: &mut Message, app_id: &[u8]) -> Result<()> {
    let mut raw = [0u8; PARAM_BYTES];
    raw[..4].copy_from_slice(&0x90000259u32.to_le_bytes());
    call.structures(&raw, PARAM_BYTES, &[(16, app_id)])
}

fn status(read: &mut Answer) -> Result<()> {
    read.status()?;
    let code = read.i32()?;
    ensure!(code == 0, Fault::Service { code, said: "Keymaster 3".into() });
    Ok(())
}

/// Consume a returned parameter vector and each embedded blob. This caller
/// does not need output authorizations, but must step over their buffers.
fn output_parameters(read: &mut Answer) -> Result<()> {
    let frame = read.pointed()?;
    ensure!(frame.len() == 16, "invalid Keymaster parameter vector frame");
    let count = u32::from_le_bytes(frame[8..12].try_into()?) as usize;
    let raw = read.pointed()?;
    ensure!(count.checked_mul(PARAM_BYTES) == Some(raw.len()), "invalid Keymaster parameter vector size");
    for entry in raw.as_chunks::<PARAM_BYTES>().0 {
        let count = u32::from_le_bytes(entry[24..28].try_into()?) as usize;
        ensure!(read.pointed()?.len() == count, "invalid Keymaster parameter blob");
    }
    Ok(())
}

#[cfg(target_os = "android")]
pub struct Keymaster3<'a> { device: skyward_binder::driver::Handle<'a> }

#[cfg(target_os = "android")]
impl<'a> Keymaster3<'a> {
    pub fn open(binder: &'a skyward_binder::driver::Binder) -> Result<Self> {
        let device = skyward_binder::hidl::find(binder, INTERFACE, "default")?.context("Keymaster 3 is not registered")?;
        Ok(Self { device })
    }

    fn begin(&self, blob: &[u8], app_id: &[u8], nonce: &[u8]) -> Result<u64> {
        ensure!(nonce.len() == 12, "AES-GCM nonce is not 12 bytes");
        let mut call = Message::call(INTERFACE);
        call.u32(1); // KeyPurpose::DECRYPT
        call.bytes(blob);
        parameters(&mut call, Some(app_id), Some(nonce))?;
        let reply = self.device.transact(BEGIN, &call).context("Keymaster 3 begin AES-GCM")?;
        let mut read = Answer::new(reply.bytes());
        status(&mut read)?;
        output_parameters(&mut read)?;
        read.u64().context("read Keymaster operation handle")
    }

    fn decrypt(&self, handle: u64, body: &[u8]) -> Result<Vec<u8>> {
        ensure!(body.len() >= 16, "AES-GCM ciphertext has no authentication tag");
        let mut consumed = 0;
        let mut output = Vec::new();
        while consumed < body.len() {
            let mut call = Message::call(INTERFACE);
            call.u64(handle);
            parameters(&mut call, None, None)?;
            call.bytes(&body[consumed..]);
            let reply = self.device.transact(UPDATE, &call).context("Keymaster 3 update AES-GCM")?;
            let mut read = Answer::new(reply.bytes());
            status(&mut read)?;
            let count = u32::try_from(read.i32()?).context("negative consumed byte count")? as usize;
            ensure!(count > 0 && count <= body.len() - consumed, "invalid Keymaster consumed byte count");
            output_parameters(&mut read)?;
            output.extend_from_slice(read.bytes()?);
            consumed += count;
        }
        let mut call = Message::call(INTERFACE);
        call.u64(handle);
        parameters(&mut call, None, None)?;
        call.bytes(&[]); // input
        call.bytes(&[]); // signature
        let reply = self.device.transact(FINISH, &call).context("Keymaster 3 authenticate AES-GCM")?;
        let mut read = Answer::new(reply.bytes());
        status(&mut read)?;
        output_parameters(&mut read)?;
        output.extend_from_slice(read.bytes()?);
        // Plaintext stays local until finish has checked the GCM tag.
        Ok(output)
    }
}

#[cfg(target_os = "android")]
impl crate::keystore::Decryptor for Keymaster3<'_> {
    fn gcm_decrypt(&self, blob: &[u8], app_id: &[u8], nonce: &[u8], body: &[u8]) -> Result<Vec<u8>> {
        let blob = hardware_blob(blob)?;
        let handle = match self.begin(blob, app_id, nonce) {
            Ok(handle) => handle,
            Err(error) if skyward_binder::status::refused_with(&error) == Some(-62) => {
                // Upgrade only the in-memory copy. The key Android keeps on
                // disk must remain usable by the installed system.
                let mut call = Message::call(INTERFACE);
                call.bytes(blob);
                upgrade_parameters(&mut call, app_id)?;
                let reply = self.device.transact(8, &call).context("Keymaster 3 in-memory key upgrade")?;
                let mut read = Answer::new(reply.bytes());
                status(&mut read).context("Keymaster 3 in-memory key upgrade")?;
                self.begin(read.bytes()?, app_id, nonce)?
            }
            Err(error) => return Err(error),
        };
        let result = self.decrypt(handle, body);
        if result.is_err() {
            let mut call = Message::call(INTERFACE);
            call.u64(handle);
            let _ = self.device.transact(ABORT, &call);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skyward_binder::parcel::Payload;

    #[test]
    fn keystore2_hardware_prefix_is_removed_without_changing_old_blobs() {
        assert_eq!(hardware_blob(b"pKMblob\0vendor key").unwrap(), b"vendor key");
        assert_eq!(hardware_blob(b"old vendor key").unwrap(), b"old vendor key");
        assert_eq!(hardware_blob(b"pKMblob\x02vendor key").unwrap(), b"pKMblob\x02vendor key");
        assert_eq!(hardware_blob(b"pKMblob").unwrap(), b"pKMblob");
        assert!(hardware_blob(b"pKMblob\x01software key").is_err());
    }

    #[test]
    fn gcm_parameters_include_real_blob_fixups_and_128_bit_authentication() {
        let mut call = Message::default();
        parameters(&mut call, Some(b"binding"), Some(&[1; 12])).unwrap();
        let offsets = call.addresses();
        assert_eq!(offsets.len(), 7);
        let data = call.values();
        let raw = &data[offsets[1] as usize..];
        let at = u64::from_le_bytes(raw[8..16].try_into().unwrap());
        let len = u64::from_le_bytes(raw[16..24].try_into().unwrap()) as usize;
        let bytes = unsafe { std::slice::from_raw_parts(at as *const u8, len) };
        assert_eq!(len, 5 * 32);
        assert_eq!(u32::from_le_bytes(bytes[64..68].try_into().unwrap()), 0x300003eb);
        assert_eq!(u64::from_le_bytes(bytes[72..80].try_into().unwrap()), 128);
        assert_eq!(u32::from_le_bytes(bytes[120..124].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(bytes[152..156].try_into().unwrap()), 12);
    }

    #[test]
    fn hal_errors_remain_distinct_from_binder_errors() {
        let mut raw = vec![0u8; 4]; raw.extend_from_slice(&(-62i32).to_le_bytes());
        let error = status(&mut Answer::new(&raw)).unwrap_err();
        assert_eq!(skyward_binder::status::refused_with(&error), Some(-62));
        assert!(status(&mut Answer::new(&[])).is_err());
    }
}
