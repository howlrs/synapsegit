//! Bounded, opaque-byte metadata inspection.  This intentionally does not
//! decode a raster or modify the caller's file.
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::Path};

pub const METADATA_SCAN_BYTES: usize = 256 * 1024;
const MAX_IFDS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageMetadataCheck { GpsFound, NoGpsFound, CouldNotCheck }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageMetadataWarning { pub role: String, pub check: ImageMetadataCheck, pub message: String }

/// Inspect only the first 256 KiB. Unsupported, malformed, and budget-limited
/// inputs are deliberately reported as `could_not_check`, never as GPS-free.
pub fn image_metadata_warning(role: &str, path: &Path) -> ImageMetadataWarning {
    let mut bytes = Vec::with_capacity(METADATA_SCAN_BYTES);
    let result = File::open(path).and_then(|mut f| f.by_ref().take((METADATA_SCAN_BYTES + 1) as u64).read_to_end(&mut bytes));
    let check = match result {
        Ok(_) if bytes.len() > METADATA_SCAN_BYTES => ImageMetadataCheck::CouldNotCheck,
        Ok(_) => inspect(&bytes),
        Err(_) => ImageMetadataCheck::CouldNotCheck,
    };
    let message = match check {
        ImageMetadataCheck::GpsFound => "Location metadata may be included in the recorded bytes. To remove it, choose a metadata-stripped copy before recording; SynapseGit does not alter the file.".into(),
        ImageMetadataCheck::NoGpsFound => "No location metadata was found in the bounded JPEG/PNG metadata check.".into(),
        ImageMetadataCheck::CouldNotCheck => "Location metadata could not be fully checked in this file; use a metadata-stripped copy if location privacy matters.".into(),
    };
    ImageMetadataWarning { role: role.into(), check, message }
}

fn inspect(b: &[u8]) -> ImageMetadataCheck {
    if b.starts_with(&[0xff,0xd8]) { return jpeg(b); }
    if b.starts_with(b"\x89PNG\r\n\x1a\n") { return png(b); }
    ImageMetadataCheck::CouldNotCheck
}
fn jpeg(b: &[u8]) -> ImageMetadataCheck {
    let mut p=2;
    while p < b.len() {
        if b[p] != 0xff { return ImageMetadataCheck::CouldNotCheck };
        while p < b.len() && b[p] == 0xff {p+=1}; if p>=b.len(){return ImageMetadataCheck::CouldNotCheck}
        let marker=b[p]; p+=1; if marker==0xd9 || marker==0xda {break}; if (0xd0..=0xd7).contains(&marker) || marker==1 {continue}
        if p+2>b.len(){return ImageMetadataCheck::CouldNotCheck}; let n=u16::from_be_bytes([b[p],b[p+1]]) as usize; p+=2;
        if n<2 || p+n-2>b.len(){return ImageMetadataCheck::CouldNotCheck}; let x=&b[p..p+n-2]; p+=n-2;
        if marker==0xe1 { if x.starts_with(b"Exif\0\0") { match exif(&x[6..]) {ImageMetadataCheck::GpsFound=>return ImageMetadataCheck::GpsFound, ImageMetadataCheck::CouldNotCheck=>return ImageMetadataCheck::CouldNotCheck, _=>{}} } else if has_xmp_gps(x) {return ImageMetadataCheck::GpsFound} }
    }
    ImageMetadataCheck::NoGpsFound
}
fn png(b: &[u8]) -> ImageMetadataCheck {
    let mut p=8;
    while p+12<=b.len() { let n=u32::from_be_bytes(b[p..p+4].try_into().unwrap()) as usize; if p+12+n>b.len(){return ImageMetadataCheck::CouldNotCheck}; let k=&b[p+4..p+8]; let x=&b[p+8..p+8+n]; p+=12+n;
        if k==b"eXIf" { match exif(x) {ImageMetadataCheck::GpsFound=>return ImageMetadataCheck::GpsFound,ImageMetadataCheck::CouldNotCheck=>return ImageMetadataCheck::CouldNotCheck,_=>{}}}
        if k==b"iTXt" && has_xmp_gps(x){return ImageMetadataCheck::GpsFound}
        if k==b"IEND" {return ImageMetadataCheck::NoGpsFound}
    }
    ImageMetadataCheck::CouldNotCheck
}
fn has_xmp_gps(bytes: &[u8]) -> bool { bytes.windows(8).any(|v| v.eq_ignore_ascii_case(b"gpslatit")) || bytes.windows(8).any(|v| v.eq_ignore_ascii_case(b"gpslongi")) }
fn exif(t: &[u8]) -> ImageMetadataCheck {
    if t.len()<8{return ImageMetadataCheck::CouldNotCheck}; let le=match &t[..2]{b"II"=>true,b"MM"=>false,_=>return ImageMetadataCheck::CouldNotCheck};
    let u16at=|o:usize|->Option<u16>{t.get(o..o+2).map(|v|if le{u16::from_le_bytes(v.try_into().unwrap())}else{u16::from_be_bytes(v.try_into().unwrap())})};
    let u32at=|o:usize|->Option<usize>{t.get(o..o+4).map(|v|if le{u32::from_le_bytes(v.try_into().unwrap())}else{u32::from_be_bytes(v.try_into().unwrap())} as usize)};
    if u16at(2)!=Some(42){return ImageMetadataCheck::CouldNotCheck}; let mut off=match u32at(4){Some(v)=>v,None=>return ImageMetadataCheck::CouldNotCheck}; let mut seen=std::collections::BTreeSet::new();
    for _ in 0..MAX_IFDS { if !seen.insert(off) || off+2>t.len(){return ImageMetadataCheck::CouldNotCheck}; let n=match u16at(off){Some(v)=>v as usize,None=>return ImageMetadataCheck::CouldNotCheck}; if n>MAX_IFDS || off+2+n*12+4>t.len(){return ImageMetadataCheck::CouldNotCheck};
      for i in 0..n {let e=off+2+i*12; let tag=u16at(e).unwrap(); if tag==0x8825 {let gps=match u32at(e+8){Some(v)=>v,None=>return ImageMetadataCheck::CouldNotCheck}; if gps>=t.len(){return ImageMetadataCheck::CouldNotCheck}; return ImageMetadataCheck::GpsFound;}}
      off=match u32at(off+2+n*12){Some(v)=>v,None=>return ImageMetadataCheck::CouldNotCheck}; if off==0{return ImageMetadataCheck::NoGpsFound}
    }
    ImageMetadataCheck::CouldNotCheck
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tiff(gps: bool, looped: bool) -> Vec<u8> {
        let mut b=b"II*\0\x08\0\0\0".to_vec(); b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&(if gps {0x8825u16}else{0x0100u16}).to_le_bytes()); b.extend_from_slice(&4u16.to_le_bytes()); b.extend_from_slice(&1u32.to_le_bytes()); b.extend_from_slice(&32u32.to_le_bytes()); b.extend_from_slice(&(if looped {8u32}else{0u32}).to_le_bytes()); b.resize(40, 0); b
    }
    fn jpeg(t: &[u8]) -> Vec<u8> { let mut b=vec![0xff,0xd8,0xff,0xe1]; b.extend_from_slice(&((t.len()+8) as u16).to_be_bytes()); b.extend_from_slice(b"Exif\0\0"); b.extend_from_slice(t); b.extend_from_slice(&[0xff,0xd9]); b }
    #[test] fn detects_jpeg_exif_gps_and_no_gps() { assert_eq!(inspect(&jpeg(&tiff(true,false))),ImageMetadataCheck::GpsFound); assert_eq!(inspect(&jpeg(&tiff(false,false))),ImageMetadataCheck::NoGpsFound); }
    #[test] fn rejects_truncated_and_looping_exif() { assert_eq!(inspect(&jpeg(b"II")),ImageMetadataCheck::CouldNotCheck); assert_eq!(inspect(&jpeg(&tiff(false,true))),ImageMetadataCheck::CouldNotCheck); }
    #[test] fn detects_png_exif_and_opaque_files_are_not_claimed_safe() { let mut p=b"\x89PNG\r\n\x1a\n".to_vec(); let t=tiff(true,false); p.extend_from_slice(&(t.len() as u32).to_be_bytes());p.extend_from_slice(b"eXIf");p.extend_from_slice(&t);p.extend_from_slice(&[0;4]);p.extend_from_slice(&0u32.to_be_bytes());p.extend_from_slice(b"IEND");p.extend_from_slice(&[0;4]); assert_eq!(inspect(&p),ImageMetadataCheck::GpsFound); assert_eq!(inspect(b"GIF89a"),ImageMetadataCheck::CouldNotCheck); }
}
