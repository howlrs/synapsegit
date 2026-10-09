//! Bounded, opaque-byte metadata inspection.  This intentionally does not
//! decode a raster or modify the caller's file.
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub const METADATA_SCAN_BYTES: usize = 256 * 1024;
/// Creator inputs have a 64 MiB ceiling. Keep the advisory bounded by the
/// same ceiling even when it is called independently of an ingest path.
pub const METADATA_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PNG_CHUNKS: usize = 4096;
const MAX_PNG_METADATA_BYTES: usize = 256 * 1024;
const MAX_IFDS: usize = 16;
const MAX_IFD_DEPTH: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageMetadataCheck {
    GpsFound,
    NoGpsFound,
    CouldNotCheck,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageMetadataWarning {
    pub role: String,
    pub check: ImageMetadataCheck,
    pub message: String,
}
pub fn metadata_warning_from_bytes(role: &str, bytes: &[u8]) -> ImageMetadataWarning {
    let check = if bytes.len() as u64 > METADATA_MAX_FILE_BYTES {
        ImageMetadataCheck::CouldNotCheck
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes)
    } else {
        inspect(
            &bytes[..bytes.len().min(METADATA_SCAN_BYTES)],
            bytes.len() > METADATA_SCAN_BYTES,
        )
    };
    warning(role, check)
}

/// Inspect opaque image metadata without decoding or changing the file.
/// JPEG headers are bounded to the first 256 KiB. PNG is structurally scanned
/// to IEND: only metadata chunks are read and CRC-checked; image payload and
/// other chunks are skipped. Unsupported, malformed, and budget-limited inputs
/// are deliberately reported as `could_not_check`, never as GPS-free.
pub fn image_metadata_warning(role: &str, path: &Path) -> ImageMetadataWarning {
    let result = open_metadata_file(path).and_then(|mut f| {
        let length = f.metadata()?;
        if !length.is_file() || length.len() > METADATA_MAX_FILE_BYTES {
            return Err(std::io::Error::other(
                "metadata input is not a regular file within the input limit",
            ));
        }
        let mut signature = [0; 8];
        let signature_len = length.len().min(signature.len() as u64) as usize;
        f.read_exact(&mut signature[..signature_len])?;
        f.seek(SeekFrom::Start(0))?;
        if signature_len == signature.len() && signature == *b"\x89PNG\r\n\x1a\n" {
            Ok(png_file(&mut f, length.len()))
        } else {
            let mut bytes = Vec::with_capacity(METADATA_SCAN_BYTES + 1);
            f.by_ref()
                .take((METADATA_SCAN_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            Ok(inspect(
                &bytes[..bytes.len().min(METADATA_SCAN_BYTES)],
                bytes.len() > METADATA_SCAN_BYTES,
            ))
        }
    });
    let check = match result {
        Ok(check) => check,
        Err(_) => ImageMetadataCheck::CouldNotCheck,
    };
    warning(role, check)
}
fn warning(role: &str, check: ImageMetadataCheck) -> ImageMetadataWarning {
    let message = match check {
        ImageMetadataCheck::GpsFound => "Location metadata may be included in the recorded bytes. To remove it, choose a metadata-stripped copy before recording; SynapseGit does not alter the file.".into(),
        ImageMetadataCheck::NoGpsFound => "No location metadata was found in the bounded JPEG/PNG metadata check.".into(),
        ImageMetadataCheck::CouldNotCheck => "Location metadata could not be fully checked in this file; use a metadata-stripped copy if location privacy matters.".into(),
    };
    ImageMetadataWarning {
        role: role.into(),
        check,
        message,
    }
}

#[cfg(unix)]
fn open_metadata_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
}

#[cfg(not(unix))]
fn open_metadata_file(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

/// `truncated` means the file continues beyond `b`. JPEG metadata segments
/// precede the first scan, so reaching it completes the header check; PNG
/// ancillary chunks may follow image data, so a truncated PNG stays unknown.
fn inspect(b: &[u8], truncated: bool) -> ImageMetadataCheck {
    if b.starts_with(&[0xff, 0xd8]) {
        return jpeg(b);
    }
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return match png(b) {
            ImageMetadataCheck::NoGpsFound if truncated => ImageMetadataCheck::CouldNotCheck,
            other => other,
        };
    }
    ImageMetadataCheck::CouldNotCheck
}
fn jpeg(b: &[u8]) -> ImageMetadataCheck {
    // MPF and Motion Photo/container XMP announce further images or media
    // after the primary image. They may carry their own location metadata.
    let mut appended_media = false;
    let mut p = 2;
    while p < b.len() {
        if b[p] != 0xff {
            return ImageMetadataCheck::CouldNotCheck;
        };
        while p < b.len() && b[p] == 0xff {
            p += 1
        }
        if p >= b.len() {
            return ImageMetadataCheck::CouldNotCheck;
        }
        let marker = b[p];
        p += 1;
        if marker == 0xd9 || marker == 0xda {
            return if appended_media {
                ImageMetadataCheck::CouldNotCheck
            } else {
                ImageMetadataCheck::NoGpsFound
            };
        };
        if (0xd0..=0xd7).contains(&marker) || marker == 1 {
            continue;
        }
        if p + 2 > b.len() {
            return ImageMetadataCheck::CouldNotCheck;
        };
        let n = u16::from_be_bytes([b[p], b[p + 1]]) as usize;
        p += 2;
        if n < 2 || p + n - 2 > b.len() {
            return ImageMetadataCheck::CouldNotCheck;
        };
        let x = &b[p..p + n - 2];
        p += n - 2;
        if marker == 0xe1 {
            if x.starts_with(b"Exif\0\0") {
                match exif(&x[6..]) {
                    ImageMetadataCheck::GpsFound => return ImageMetadataCheck::GpsFound,
                    ImageMetadataCheck::CouldNotCheck => return ImageMetadataCheck::CouldNotCheck,
                    _ => {}
                }
            } else if has_xmp_gps(x) {
                return ImageMetadataCheck::GpsFound;
            } else if declares_appended_media(x) {
                appended_media = true;
            }
        } else if marker == 0xe2 && x.starts_with(b"MPF\0") {
            appended_media = true;
        }
    }
    ImageMetadataCheck::CouldNotCheck
}
fn declares_appended_media(xmp: &[u8]) -> bool {
    [
        b"photos/1.0/container".as_slice(),
        b"motionphoto",
        b"microvideo",
    ]
    .iter()
    .any(|needle| contains_ignore_ascii_case(xmp, needle))
}
fn contains_ignore_ascii_case(bytes: &[u8], needle: &[u8]) -> bool {
    bytes
        .windows(needle.len())
        .any(|value| value.eq_ignore_ascii_case(needle))
}
fn png(b: &[u8]) -> ImageMetadataCheck {
    let mut p = 8;
    let mut chunks = 0;
    let mut metadata_bytes = 0usize;
    let mut gps_found = false;
    while p + 12 <= b.len() {
        chunks += 1;
        if chunks > MAX_PNG_CHUNKS {
            return ImageMetadataCheck::CouldNotCheck;
        }
        let n = u32::from_be_bytes(b[p..p + 4].try_into().unwrap()) as usize;
        let Some(end) = p.checked_add(12 + n) else {
            return ImageMetadataCheck::CouldNotCheck;
        };
        if end > b.len() {
            return ImageMetadataCheck::CouldNotCheck;
        };
        let k = &b[p + 4..p + 8];
        let x = &b[p + 8..p + 8 + n];
        let metadata = matches!(k, b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt");
        if metadata {
            metadata_bytes = match metadata_bytes.checked_add(n) {
                Some(value) if value <= MAX_PNG_METADATA_BYTES => value,
                _ => return ImageMetadataCheck::CouldNotCheck,
            };
            let expected = u32::from_be_bytes(b[p + 8 + n..end].try_into().unwrap());
            if png_crc(k, x) != expected {
                return ImageMetadataCheck::CouldNotCheck;
            }
        }
        p = end;
        if metadata {
            match png_metadata_check(k, x) {
                ImageMetadataCheck::GpsFound => gps_found = true,
                ImageMetadataCheck::CouldNotCheck => return ImageMetadataCheck::CouldNotCheck,
                ImageMetadataCheck::NoGpsFound => {}
            }
        }
        if k == b"IEND" {
            return if n == 0 && p == b.len() {
                if gps_found {
                    ImageMetadataCheck::GpsFound
                } else {
                    ImageMetadataCheck::NoGpsFound
                }
            } else {
                ImageMetadataCheck::CouldNotCheck
            };
        }
    }
    ImageMetadataCheck::CouldNotCheck
}

/// PNG structure is read by offset so a large IDAT never becomes an in-memory
/// metadata scan. Only eXIf/text payloads and their CRCs are inspected.
fn png_file(file: &mut File, length: u64) -> ImageMetadataCheck {
    if length < 20 {
        return ImageMetadataCheck::CouldNotCheck;
    }
    let mut position = 8u64;
    let mut chunks = 0usize;
    let mut metadata_bytes = 0usize;
    let mut gps_found = false;
    loop {
        chunks += 1;
        if chunks > MAX_PNG_CHUNKS || position.checked_add(12).is_none_or(|end| end > length) {
            return ImageMetadataCheck::CouldNotCheck;
        }
        if file.seek(SeekFrom::Start(position)).is_err() {
            return ImageMetadataCheck::CouldNotCheck;
        }
        let mut header = [0; 8];
        if file.read_exact(&mut header).is_err() {
            return ImageMetadataCheck::CouldNotCheck;
        }
        let payload_length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let kind = &header[4..];
        let Some(end) = position.checked_add(12 + payload_length as u64) else {
            return ImageMetadataCheck::CouldNotCheck;
        };
        if end > length {
            return ImageMetadataCheck::CouldNotCheck;
        }
        let metadata = matches!(kind, b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt");
        if metadata {
            metadata_bytes = match metadata_bytes.checked_add(payload_length) {
                Some(value) if value <= MAX_PNG_METADATA_BYTES => value,
                _ => return ImageMetadataCheck::CouldNotCheck,
            };
            let mut payload = vec![0; payload_length];
            let mut crc = [0; 4];
            if file.read_exact(&mut payload).is_err()
                || file.read_exact(&mut crc).is_err()
                || png_crc(kind, &payload) != u32::from_be_bytes(crc)
            {
                return ImageMetadataCheck::CouldNotCheck;
            }
            match png_metadata_check(kind, &payload) {
                ImageMetadataCheck::GpsFound => gps_found = true,
                ImageMetadataCheck::CouldNotCheck => return ImageMetadataCheck::CouldNotCheck,
                ImageMetadataCheck::NoGpsFound => {}
            }
        }
        position = end;
        if kind == b"IEND" {
            return if payload_length == 0 && position == length {
                if gps_found {
                    ImageMetadataCheck::GpsFound
                } else {
                    ImageMetadataCheck::NoGpsFound
                }
            } else {
                ImageMetadataCheck::CouldNotCheck
            };
        }
    }
}

fn png_metadata_check(kind: &[u8], payload: &[u8]) -> ImageMetadataCheck {
    if kind == b"eXIf" {
        return exif(payload);
    }
    if matches!(kind, b"tEXt" | b"zTXt" | b"iTXt") && is_raw_metadata_profile(payload) {
        return ImageMetadataCheck::CouldNotCheck;
    }
    if kind == b"iTXt" {
        return match itxt_xmp(payload) {
            Some(Ok(payload)) if has_xmp_gps(payload) => ImageMetadataCheck::GpsFound,
            Some(Ok(_)) | None => ImageMetadataCheck::NoGpsFound,
            Some(Err(())) => ImageMetadataCheck::CouldNotCheck,
        };
    }
    ImageMetadataCheck::NoGpsFound
}
fn png_crc(kind: &[u8], data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in kind.iter().chain(data) {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
fn is_raw_metadata_profile(chunk: &[u8]) -> bool {
    let keyword = chunk.split(|byte| *byte == 0).next().unwrap_or_default();
    [
        b"raw profile type exif".as_slice(),
        b"raw profile type app1",
        b"raw profile type xmp",
    ]
    .iter()
    .any(|name| keyword.eq_ignore_ascii_case(name))
}
fn has_xmp_gps(bytes: &[u8]) -> bool {
    contains_ignore_ascii_case(bytes, b"gpslatit") || contains_ignore_ascii_case(bytes, b"gpslongi")
}
fn itxt_xmp(bytes: &[u8]) -> Option<Result<&[u8], ()>> {
    let Some(key_end) = bytes.iter().position(|b| *b == 0) else {
        return Some(Err(()));
    };
    if !bytes[..key_end].eq_ignore_ascii_case(b"xml:com.adobe.xmp") {
        return None;
    }
    let rest = &bytes[key_end + 1..];
    if rest.len() < 2 || rest[0] != 0 || rest[1] != 0 {
        return Some(Err(()));
    }
    let Some(language_end) = rest[2..].iter().position(|b| *b == 0) else {
        return Some(Err(()));
    };
    let language_end = language_end + 2;
    let Some(translated_end) = rest[language_end + 1..].iter().position(|b| *b == 0) else {
        return Some(Err(()));
    };
    let text_start = translated_end + language_end + 2;
    Some(Ok(&rest[text_start..]))
}
fn exif(t: &[u8]) -> ImageMetadataCheck {
    if t.len() < 8 {
        return ImageMetadataCheck::CouldNotCheck;
    };
    let le = match &t[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return ImageMetadataCheck::CouldNotCheck,
    };
    let u16at = |o: usize| -> Option<u16> {
        t.get(o..o + 2).map(|v| {
            if le {
                u16::from_le_bytes(v.try_into().unwrap())
            } else {
                u16::from_be_bytes(v.try_into().unwrap())
            }
        })
    };
    let u32at = |o: usize| -> Option<usize> {
        t.get(o..o+4).map(|v|if le{u32::from_le_bytes(v.try_into().unwrap())}else{u32::from_be_bytes(v.try_into().unwrap())} as usize)
    };
    if u16at(2) != Some(42) {
        return ImageMetadataCheck::CouldNotCheck;
    };
    let first = match u32at(4) {
        Some(v) => v,
        None => return ImageMetadataCheck::CouldNotCheck,
    };
    let mut pending = vec![(first, 0usize)];
    let mut seen = std::collections::BTreeSet::new();
    while let Some((off, depth)) = pending.pop() {
        if depth > MAX_IFD_DEPTH || !seen.insert(off) || seen.len() > MAX_IFDS || off + 2 > t.len()
        {
            return ImageMetadataCheck::CouldNotCheck;
        };
        let n = match u16at(off) {
            Some(v) => v as usize,
            None => return ImageMetadataCheck::CouldNotCheck,
        };
        if off
            .checked_add(2 + n * 12 + 4)
            .is_none_or(|end| end > t.len())
        {
            return ImageMetadataCheck::CouldNotCheck;
        };
        for i in 0..n {
            let e = off + 2 + i * 12;
            let tag = u16at(e).unwrap();
            let value = match u32at(e + 8) {
                Some(v) => v,
                None => return ImageMetadataCheck::CouldNotCheck,
            };
            if tag == 0x8825 {
                if value >= t.len() {
                    return ImageMetadataCheck::CouldNotCheck;
                };
                return ImageMetadataCheck::GpsFound;
            }
            if matches!(tag, 0x8769 | 0xa005) {
                pending.push((value, depth + 1))
            } else if tag == 0x014a {
                let count = match u32at(e + 4) {
                    Some(v) => v,
                    None => return ImageMetadataCheck::CouldNotCheck,
                };
                if count > MAX_IFDS {
                    return ImageMetadataCheck::CouldNotCheck;
                };
                // A single 4-byte offset is stored inline in the entry.
                if count == 1 {
                    pending.push((value, depth + 1));
                    continue;
                }
                for j in 0..count {
                    let child = match u32at(value + j * 4) {
                        Some(v) => v,
                        None => return ImageMetadataCheck::CouldNotCheck,
                    };
                    pending.push((child, depth + 1));
                }
            }
        }
        let next = match u32at(off + 2 + n * 12) {
            Some(v) => v,
            None => return ImageMetadataCheck::CouldNotCheck,
        };
        if next != 0 {
            pending.push((next, depth))
        }
    }
    ImageMetadataCheck::NoGpsFound
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png_chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut bytes = (data.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(data);
        bytes.extend_from_slice(&png_crc(kind, data).to_be_bytes());
        bytes
    }
    fn png(chunks: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        for chunk in chunks {
            bytes.extend_from_slice(&chunk);
        }
        bytes
    }
    fn tiff(gps: bool, looped: bool) -> Vec<u8> {
        let mut b = b"II*\0\x08\0\0\0".to_vec();
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&(if gps { 0x8825u16 } else { 0x0100u16 }).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&32u32.to_le_bytes());
        b.extend_from_slice(&(if looped { 8u32 } else { 0u32 }).to_le_bytes());
        b.resize(40, 0);
        b
    }
    fn jpeg(t: &[u8]) -> Vec<u8> {
        let mut b = vec![0xff, 0xd8, 0xff, 0xe1];
        b.extend_from_slice(&((t.len() + 8) as u16).to_be_bytes());
        b.extend_from_slice(b"Exif\0\0");
        b.extend_from_slice(t);
        b.extend_from_slice(&[0xff, 0xd9]);
        b
    }
    #[test]
    fn detects_jpeg_exif_gps_and_no_gps() {
        assert_eq!(
            inspect(&jpeg(&tiff(true, false)), false),
            ImageMetadataCheck::GpsFound
        );
        assert_eq!(
            inspect(&jpeg(&tiff(false, false)), false),
            ImageMetadataCheck::NoGpsFound
        );
    }
    #[test]
    fn rejects_truncated_and_looping_exif() {
        assert_eq!(
            inspect(&jpeg(b"II"), false),
            ImageMetadataCheck::CouldNotCheck
        );
        assert_eq!(
            inspect(&jpeg(&tiff(false, true)), false),
            ImageMetadataCheck::CouldNotCheck
        );
    }
    #[test]
    fn incomplete_containers_are_not_gps_free() {
        assert_eq!(
            inspect(&[0xff, 0xd8], false),
            ImageMetadataCheck::CouldNotCheck
        );
        assert_eq!(
            inspect(b"\x89PNG\r\n\x1a\n", false),
            ImageMetadataCheck::CouldNotCheck
        );
    }
    #[test]
    fn a_complete_jpeg_header_is_checked_even_when_image_data_continues() {
        assert_eq!(
            inspect(&jpeg(&tiff(true, false)), true),
            ImageMetadataCheck::GpsFound
        );
        // Typical phone photos are far larger than the scan prefix; their
        // metadata segments still end before the first scan.
        let mut large = jpeg(&tiff(false, false));
        large.truncate(large.len() - 2);
        large.extend_from_slice(&[0xff, 0xda, 0, 8, 1, 1, 0, 0, 0x3f, 0]);
        large.resize(METADATA_SCAN_BYTES + 1024, 0x11);
        assert_eq!(
            metadata_warning_from_bytes("original", &large).check,
            ImageMetadataCheck::NoGpsFound
        );
    }
    #[test]
    fn appended_images_or_media_keep_jpeg_unknown() {
        let segment = |marker: u8, payload: &[u8]| {
            let mut bytes = vec![0xff, marker];
            bytes.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            bytes.extend_from_slice(payload);
            bytes
        };
        for extra in [
            segment(0xe2, b"MPF\0MM\0*"),
            segment(
                0xe1,
                b"http://ns.adobe.com/xap/1.0/\0<x xmlns:Container=\"http://ns.google.com/photos/1.0/container/\"/>",
            ),
            segment(0xe1, b"http://ns.adobe.com/xap/1.0/\0GCamera:MotionPhoto=\"1\""),
        ] {
            let mut bytes = vec![0xff, 0xd8];
            bytes.extend_from_slice(&extra);
            bytes.extend_from_slice(&[0xff, 0xd9]);
            assert_eq!(inspect(&bytes, false), ImageMetadataCheck::CouldNotCheck);
        }
    }
    #[test]
    fn png_after_the_prefix_and_raw_profiles_stay_unknown() {
        let mut plain = b"\x89PNG\r\n\x1a\n".to_vec();
        plain.extend_from_slice(&png_chunk(b"IEND", b""));
        assert_eq!(inspect(&plain, false), ImageMetadataCheck::NoGpsFound);
        assert_eq!(inspect(&plain, true), ImageMetadataCheck::CouldNotCheck);
        let mut raw = b"\x89PNG\r\n\x1a\n".to_vec();
        raw.extend_from_slice(&png_chunk(b"zTXt", b"Raw profile type exif\0\0x"));
        raw.extend_from_slice(&png_chunk(b"IEND", b""));
        assert_eq!(inspect(&raw, false), ImageMetadataCheck::CouldNotCheck);
    }
    #[test]
    fn scans_large_png_metadata_after_idat_without_reading_image_payload() {
        let xmp = b"XML:com.adobe.xmp\0\0\0\0\0GPSLatitude";
        let bytes = png([
            png_chunk(b"IDAT", &vec![0; METADATA_SCAN_BYTES + 1]),
            png_chunk(b"iTXt", xmp),
            png_chunk(b"IEND", b""),
        ]);
        assert_eq!(
            metadata_warning_from_bytes("original", &bytes).check,
            ImageMetadataCheck::GpsFound
        );
        let path = std::env::temp_dir().join(format!(
            "synapsegit-metadata-{}-{}.png",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            image_metadata_warning("original", &path).check,
            ImageMetadataCheck::GpsFound
        );
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn short_complete_jpeg_file_matches_the_byte_check() {
        let bytes = [0xff, 0xd8, 0xff, 0xd9];
        assert_eq!(
            metadata_warning_from_bytes("original", &bytes).check,
            ImageMetadataCheck::NoGpsFound
        );
        let path = std::env::temp_dir().join(format!(
            "synapsegit-short-jpeg-{}-{}.jpg",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            image_metadata_warning("original", &path).check,
            ImageMetadataCheck::NoGpsFound
        );
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn png_limits_crc_and_trailing_bytes_are_not_gps_free() {
        let clean = png([
            png_chunk(b"IDAT", &vec![0; METADATA_SCAN_BYTES + 1]),
            png_chunk(b"IEND", b""),
        ]);
        assert_eq!(
            metadata_warning_from_bytes("original", &clean).check,
            ImageMetadataCheck::NoGpsFound
        );
        let oversized = png([
            png_chunk(b"tEXt", &vec![b'x'; MAX_PNG_METADATA_BYTES + 1]),
            png_chunk(b"IEND", b""),
        ]);
        assert_eq!(
            metadata_warning_from_bytes("original", &oversized).check,
            ImageMetadataCheck::CouldNotCheck
        );
        let mut too_many_chunks = b"\x89PNG\r\n\x1a\n".to_vec();
        for _ in 0..=MAX_PNG_CHUNKS {
            too_many_chunks.extend_from_slice(&png_chunk(b"IDAT", b""));
        }
        too_many_chunks.extend_from_slice(&png_chunk(b"IEND", b""));
        assert_eq!(
            metadata_warning_from_bytes("original", &too_many_chunks).check,
            ImageMetadataCheck::CouldNotCheck
        );
        let mut trailing = clean.clone();
        trailing.push(0);
        assert_eq!(inspect(&trailing, false), ImageMetadataCheck::CouldNotCheck);
        let mut gps_with_trailing = png([
            png_chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0GPSLatitude"),
            png_chunk(b"IEND", b""),
        ]);
        gps_with_trailing.push(0);
        assert_eq!(
            inspect(&gps_with_trailing, false),
            ImageMetadataCheck::CouldNotCheck
        );
        let mut corrupt_chunk = png_chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0x");
        let last = corrupt_chunk.len() - 1;
        corrupt_chunk[last] ^= 1;
        let corrupt = png([corrupt_chunk, png_chunk(b"IEND", b"")]);
        assert_eq!(inspect(&corrupt, false), ImageMetadataCheck::CouldNotCheck);
    }
    #[test]
    fn a_single_sub_ifd_offset_is_read_inline() {
        let mut t = tiff(false, false);
        t.resize(80, 0);
        t[10..12].copy_from_slice(&0x014au16.to_le_bytes());
        t[14..18].copy_from_slice(&1u32.to_le_bytes());
        t[18..22].copy_from_slice(&40u32.to_le_bytes());
        t[40..42].copy_from_slice(&1u16.to_le_bytes());
        t[42..44].copy_from_slice(&0x8825u16.to_le_bytes());
        t[44..46].copy_from_slice(&4u16.to_le_bytes());
        t[46..50].copy_from_slice(&1u32.to_le_bytes());
        t[50..54].copy_from_slice(&60u32.to_le_bytes());
        assert_eq!(inspect(&jpeg(&t), false), ImageMetadataCheck::GpsFound);
    }
    #[test]
    fn follows_nested_exif_ifd_and_allows_large_entry_count() {
        let mut t = tiff(false, false);
        t.resize(600, 0);
        t[8] = 17;
        t[9] = 0;
        for i in 0..17 {
            let e = 10 + i * 12;
            t[e..e + 2].copy_from_slice(&0x0100u16.to_le_bytes());
            t[e + 2..e + 4].copy_from_slice(&4u16.to_le_bytes());
            t[e + 4..e + 8].copy_from_slice(&1u32.to_le_bytes());
        }
        let e = 10;
        t[e..e + 2].copy_from_slice(&0x8769u16.to_le_bytes());
        t[e + 8..e + 12].copy_from_slice(&300u32.to_le_bytes());
        t[214..218].copy_from_slice(&0u32.to_le_bytes());
        t[300..302].copy_from_slice(&1u16.to_le_bytes());
        t[302..304].copy_from_slice(&0x8825u16.to_le_bytes());
        t[304..306].copy_from_slice(&4u16.to_le_bytes());
        t[306..310].copy_from_slice(&1u32.to_le_bytes());
        t[310..314].copy_from_slice(&400u32.to_le_bytes());
        assert_eq!(inspect(&jpeg(&t), false), ImageMetadataCheck::GpsFound);
    }
    #[test]
    fn detects_png_exif_and_opaque_files_are_not_claimed_safe() {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        let t = tiff(true, false);
        p.extend_from_slice(&(t.len() as u32).to_be_bytes());
        p.extend_from_slice(b"eXIf");
        p.extend_from_slice(&t);
        p.extend_from_slice(&png_crc(b"eXIf", &t).to_be_bytes());
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(b"IEND");
        p.extend_from_slice(&png_crc(b"IEND", b"").to_be_bytes());
        assert_eq!(inspect(&p, false), ImageMetadataCheck::GpsFound);
        assert_eq!(inspect(b"GIF89a", false), ImageMetadataCheck::CouldNotCheck);
    }
}
