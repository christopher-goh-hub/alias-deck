//! Approved pack bytes, content-addressed. A build reads only these for git packs and re-hashes
//! them, so a changed file fails loudly instead of compiling bytes nobody approved.

use std::fs;

use alias_deck_core::Sha256;
use sha2::Digest;

use crate::fsx::write_atomic;
use crate::{Error, Paths};

pub fn sha256(bytes: &[u8]) -> Sha256 {
    Sha256::from_digest(sha2::Sha256::digest(bytes).into())
}

/// Idempotent: the same bytes always land at the same path.
pub fn put(paths: &Paths, text: &str) -> Result<Sha256, Error> {
    let sha = sha256(text.as_bytes());
    let path = paths.store_pack(&sha);
    if fs::read(&path).map(|b| sha256(&b) == sha).unwrap_or(false) {
        return Ok(sha);
    }
    write_atomic(&path, text.as_bytes())?;
    Ok(sha)
}

pub fn get(paths: &Paths, sha: &Sha256) -> Result<String, Error> {
    let path = paths.store_pack(sha);
    let bytes = fs::read(&path).map_err(Error::io(&path))?;
    let actual = sha256(&bytes);
    if &actual != sha {
        return Err(Error::Tampered { path, expected: sha.clone(), actual });
    }
    String::from_utf8(bytes).map_err(|e| Error::io(&path)(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
}
