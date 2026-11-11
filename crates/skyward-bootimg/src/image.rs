use abootimg_oxide::{binrw::io::Cursor, Header, HeaderV0Versioned, HeaderV3};
use sha1::{Digest, Sha1};
use anyhow::{anyhow, ensure, Context, Result};
use std::fs;
use std::path::Path;

/// The page a version 3 or 4 header pads its sections to. Version 0 to 2
/// headers carry their own page size, which is the same 4096 on the older
/// Qualcomm recoveries but is not on every one of them.
const PAGE_V3: usize = 4096;

/// A boot image split into its header, kernel and ramdisk. Version 3 and 4
/// headers hold the ramdisk-only layout a HyperOS recovery ships; version 0
/// to 2 hold the kernel and ramdisk beside each other, with the board's
/// device trees riding at the end of the kernel section.
pub struct BootImage {
    pub header: Header,
    pub kernel: Vec<u8>,
    pub ramdisk: Vec<u8>,
}

fn section(bytes: &[u8], at: usize, len: u32, what: &str) -> Result<Vec<u8>> {
    let end = at.checked_add(len as usize).context("section size overflow")?;
    bytes.get(at..end).map(<[u8]>::to_vec).ok_or_else(|| anyhow!("{what} runs past the end of the image"))
}

/// The page `header` pads its sections to.
fn page(header: &Header) -> Result<usize> {
    let pg = match header {
        Header::V0(h) => h.page_size as usize,
        Header::V3(_) => PAGE_V3,
    };
    ensure!(pg.is_power_of_two() && (2048..=65536).contains(&pg), "invalid boot image page size {pg}");
    Ok(pg)
}

/// This representation only carries kernel and ramdisk. Refuse other
/// header-declared sections rather than losing a board's device trees.
fn supported(header: &Header) -> Result<()> {
    if let Header::V0(h) = header {
        ensure!(h.second_bootloader_size == 0, "second-stage bootloader sections are not supported");
        let (dtbo, dtb) = match h.versioned {
            HeaderV0Versioned::V0 => (0, 0),
            HeaderV0Versioned::V1 { recovery_dtbo_size, .. } => (recovery_dtbo_size, 0),
            HeaderV0Versioned::V2 { recovery_dtbo_size, dtb_size, .. } => (recovery_dtbo_size, dtb_size),
        };
        ensure!(dtbo == 0 && dtb == 0, "separate recovery DTBO/DTB sections are not supported; appended kernel DTBs are preserved");
    }
    Ok(())
}

/// Where a header's kernel and ramdisk sections start.
fn positions(header: &Header) -> (usize, usize) {
    match header {
        Header::V0(h) => (h.kernel_position(), h.ramdisk_position()),
        Header::V3(h) => (HeaderV3::kernel_position(), h.ramdisk_position()),
    }
}

/// A header's kernel and ramdisk sizes.
fn sizes(header: &Header) -> (u32, u32) {
    match header {
        Header::V0(h) => (h.kernel_size, h.ramdisk_size),
        Header::V3(h) => (h.kernel_size, h.ramdisk_size),
    }
}

impl BootImage {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        BootImage::parse(&bytes).with_context(|| format!("read {}", path.display()))
    }

    /// The image `bytes` hold, as a partition holds it: the image, then
    /// whatever fills the partition after it. Version 0 to 2 headers carry
    /// the kernel and ramdisk beside each other; version 3 and 4 hold the
    /// ramdisk alone.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let header = match Header::parse(&mut Cursor::new(bytes)) {
            Ok(h) => h,
            Err(e) => return Err(anyhow!(e).context("header")),
        };
        let pg = page(&header)?;
        supported(&header)?;
        let (kernel_at, ramdisk_at) = positions(&header);
        let (kernel_len, ramdisk_len) = sizes(&header);
        let end = ramdisk_at.checked_add(ramdisk_len as usize).context("ramdisk size overflow")?;
        let after = end.div_ceil(pg).checked_mul(pg).context("image size overflow")?;
        if matches!(&header,
            Header::V0(_)) && after < bytes.len() {
            let tail = &bytes[after..];
            // A changed image cannot retain its old AVB1 BootSignature.
            // Accept only its known ASN.1 layout; arbitrary appended data
            // may be a section this representation cannot safely relocate.
            ensure!(tail.iter().all(|&b| b == 0) || boot_signature(tail, after).is_some(),
                "unrecognised data after the legacy ramdisk");
        }
        let kernel = section(bytes, kernel_at, kernel_len, "kernel")?;
        let ramdisk = section(bytes, ramdisk_at, ramdisk_len, "ramdisk")?;
        Ok(BootImage { header, kernel, ramdisk })
    }

    /// The image with sizes taken from its parts. A version 4 header keeps its
    /// version but carries no boot signature; unlocked bootloaders do not ask
    /// for one.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut header = self.header.clone();
        match &mut header {
            Header::V0(h) => {
                h.kernel_size = u32::try_from(self.kernel.len()).context("kernel size")?;
                h.ramdisk_size = u32::try_from(self.ramdisk.len()).context("ramdisk size")?;
                let mut hash = Sha1::new();
                hash.update(&self.kernel);
                hash.update(h.kernel_size.to_le_bytes());
                hash.update(&self.ramdisk);
                hash.update(h.ramdisk_size.to_le_bytes());
                hash.update(0u32.to_le_bytes()); // second stage size, including when absent
                if h.header_version() >= 1 { hash.update(0u32.to_le_bytes()); }
                if h.header_version() >= 2 { hash.update(0u32.to_le_bytes()); }
                h.hash_digest = [0; 32];
                h.hash_digest[..20].copy_from_slice(&hash.finalize());
            }
            Header::V3(h) => {
                h.kernel_size = u32::try_from(self.kernel.len()).context("kernel size")?;
                h.ramdisk_size = u32::try_from(self.ramdisk.len()).context("ramdisk size")?;
                h.v4_signature_size = h.v4_signature_size.map(|_| 0);
            }
        }
        let pg = page(&header)?;
        supported(&header)?;
        let mut out = Vec::new();
        header.write(&mut out).context("write header")?;
        pad(&mut out, pg);
        out.extend_from_slice(&self.kernel);
        pad(&mut out, pg);
        out.extend_from_slice(&self.ramdisk);
        pad(&mut out, pg);
        Ok(out)
    }
}

fn pad(out: &mut Vec<u8>, page: usize) {
    out.resize(out.len().div_ceil(page) * page, 0);
}

/// Read one definite-length DER element, leaving the following elements.
fn der<'a>(input: &mut &'a [u8], tag: u8) -> Option<&'a [u8]> {
    if *input.first()? != tag { return None; }
    let first = *input.get(1)?;
    let (len, offset) = if first < 128 {
        (usize::from(first), 2)
    } else {
        let count = usize::from(first & 127);
        if count == 0 || count > 4 { return None; }
        let mut n = 0usize;
        for b in input.get(2..2 + count)? { n = n.checked_mul(256)?.checked_add(usize::from(*b))?; }
        (n, 2 + count)
    };
    let end = offset.checked_add(len)?;
    let body = input.get(offset..end)?;
    *input = input.get(end..)?;
    Some(body)
}

/// Recognise the Android AVB1 signature envelope and its authenticated
/// target and image length. This is structural recognition, not signature
/// verification; rebuilt images are unsigned and require an unlocked device.
fn boot_signature(mut tail: &[u8], image_len: usize) -> Option<()> {
    let mut body = der(&mut tail, 0x30)?;
    if !tail.iter().all(|&b| b == 0) || der(&mut body, 0x02)? != [1] { return None; }
    let mut cert = der(&mut body, 0x30)?;
    der(&mut cert, 0x30)?;
    der(&mut cert, 0x30)?;
    der(&mut cert, 0x03)?;
    if !cert.is_empty() { return None; }
    let mut alg = der(&mut body, 0x30)?;
    der(&mut alg, 0x06)?;
    if !alg.is_empty() { der(&mut alg, 0x05)?; }
    if !alg.is_empty() { return None; }
    let mut attrs = der(&mut body, 0x30)?;
    let target = der(&mut attrs, 0x13)?;
    if target != b"/boot" && target != b"/recovery" { return None; }
    let size = der(&mut attrs, 0x02)?;
    if size.is_empty() || size[0] & 0x80 != 0 { return None; }
    let mut value = 0usize;
    for b in size { value = value.checked_mul(256)?.checked_add(usize::from(*b))?; }
    if !attrs.is_empty() || value != image_len || der(&mut body, 0x04)?.is_empty() || !body.is_empty() { return None; }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Format, Ramdisk};

    fn legacy(version: u32, pg: u32) -> Vec<u8> {
        let mut bytes = vec![0; 12288];
        bytes[..8].copy_from_slice(b"ANDROID!");
        for (at, value) in [(8, 6u32), (12, 0x80008000), (16, 7), (20, 0x82000000),
            (32, 0x80000100), (36, pg), (40, version)] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[48..53].copy_from_slice(b"board");
        bytes[64..79].copy_from_slice(b"console=ttyMSM0");
        if version >= 1 { bytes[1644..1648].copy_from_slice(&(if version == 1 { 1648u32 } else { 1660 }).to_le_bytes()); }
        bytes[4096..4102].copy_from_slice(b"kernel");
        bytes[8192..8199].copy_from_slice(b"ramdisk");
        bytes
    }

    #[test]
    fn legacy_repacking_preserves_board_addresses_and_cmdline_and_rebuilds_id() {
        for version in [0, 1, 2] {
            let raw = legacy(version, 4096);
            let mut image = BootImage::parse(&raw).unwrap();
            image.ramdisk = Ramdisk { entries: Vec::new() }.encode_as(Format::Gzip).unwrap();
            let packed = image.to_bytes().unwrap();
            let back = BootImage::parse(&packed).unwrap();
            assert_eq!(back.kernel, b"kernel");
            assert_eq!(Format::of(&back.ramdisk).unwrap(), Format::Gzip);
            assert!(Ramdisk::decode(&back.ramdisk).unwrap().entries.is_empty());
            assert_eq!(&packed[12..16], &raw[12..16]);
            assert_eq!(&packed[20..24], &raw[20..24]);
            assert_eq!(&packed[32..80], &raw[32..80]);
            assert_eq!(back.header.header_version(), version);
            assert_eq!(packed.len() % 4096, 0);
            assert!(packed[576..596].iter().any(|&b| b != 0));
            assert_eq!(&packed[596..608], &[0u8; 12]);
            // A second rebuild is byte-for-byte stable, including its ID.
            assert_eq!(packed, back.to_bytes().unwrap());
        }
    }

    #[test]
    fn legacy_id_hashes_the_version_specific_absent_section_sizes() {
        for (version, expected) in [
            (0, "5d017bf93d2f03723918ab9e296e37f6f72c2b5f"),
            (1, "e3dac0a35a1dacb0ea9e8df2c4f06023d763c01f"),
            (2, "97f223923d1121ef67e3cdfc15d46064e0b7cfb2"),
        ] {
            let packed = BootImage::parse(&legacy(version, 4096)).unwrap().to_bytes().unwrap();
            let actual: String = packed[576..596].iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(actual, expected);
        }
    }

    fn der_element(tag: u8, body: &[u8]) -> Vec<u8> {
        assert!(body.len() < 128);
        [vec![tag, body.len() as u8], body.to_vec()].concat()
    }

    fn signature(size: u32) -> Vec<u8> {
        let cert = der_element(0x30, &[0x30, 0, 0x30, 0, 0x03, 1, 0]);
        let attrs = der_element(0x30, &[der_element(0x13, b"/recovery"), der_element(0x02, &size.to_be_bytes())].concat());
        der_element(0x30, &[vec![2, 1, 1], cert, vec![0x30, 3, 6, 1, 0x2a], attrs, vec![4, 1, 1]].concat())
    }

    #[test]
    fn only_a_recognised_legacy_signature_may_follow_the_ramdisk() {
        let raw = legacy(1, 4096);
        let mut signed = raw.clone();
        signed.extend(signature(raw.len() as u32));
        signed.extend([0; 8]);
        assert_eq!(BootImage::parse(&signed).unwrap().to_bytes().unwrap().len(), raw.len());
        signed.push(1);
        assert!(BootImage::parse(&signed).is_err());
        let mut wrong_length = raw.clone();
        wrong_length.extend(signature(1));
        assert!(BootImage::parse(&wrong_length).is_err());
        let mut unknown = raw;
        unknown.extend(b"unknown extra section");
        assert!(BootImage::parse(&unknown).is_err());
    }

    #[test]
    fn malformed_sizes_and_declared_secondary_sections_are_errors() {
        for pg in [0, 1, 512, 4095, u32::MAX] {
            assert!(BootImage::parse(&legacy(1, pg)).is_err(), "page size {pg}");
        }
        let mut second = legacy(1, 4096);
        second[24..28].copy_from_slice(&1u32.to_le_bytes());
        assert!(BootImage::parse(&second).is_err());
        second[24..28].fill(0);
        second[1632..1636].copy_from_slice(&1u32.to_le_bytes());
        assert!(BootImage::parse(&second).is_err());
        let raw = legacy(1, 4096);
        for length in [0, 8, 40, 1647, 4095, 8198] {
            assert!(BootImage::parse(&raw[..length]).is_err(), "truncated at {length}");
        }
    }

    #[test]
    fn modern_v3_and_v4_keep_their_layout_and_version() {
        for version in [3, 4] {
            let h = HeaderV3 {
                kernel_size: 0, ramdisk_size: 3,
                osversionpatch: abootimg_oxide::OsVersionPatch(0),
                cmdline: Box::new([0; 1536]),
                v4_signature_size: (version == 4).then_some(0),
            };
            let image = BootImage { header: Header::V3(h), kernel: Vec::new(), ramdisk: b"rd!".to_vec() };
            let back = BootImage::parse(&image.to_bytes().unwrap()).unwrap();
            assert_eq!(back.header.header_version(), version);
            assert_eq!(back.ramdisk, b"rd!");
        }
    }
}
