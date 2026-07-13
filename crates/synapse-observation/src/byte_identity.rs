//! Pure byte-identity decision used by the first-party adapter.
//!
//! This source file is compiled as the algorithm and embedded verbatim as the
//! implementation Blob. Equality is evaluated over already verified Blob OIDs;
//! it makes no claim about decoded media or the observed physical subject.

pub(crate) fn media_oids_are_identical(base_media_oid: &str, target_media_oid: &str) -> bool {
    base_media_oid == target_media_oid
}
