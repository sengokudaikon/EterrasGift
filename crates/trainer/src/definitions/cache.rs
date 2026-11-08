//! Bounded, versioned definition persistence. A cache is captured data, not proof
//! that definitions still belong to a running session. No process is accessed.
//!
//! The key hashes length-delimited identity fields, including extraction version.
//! The file contains that identity, literal limits, and the catalog's entries in
//! their form (`entries`), followed by an ASCII SHA-256 of all preceding bytes.
//! What it holds is checked again as every catalog is (`GameCatalog::new`). A file
//! of another schema is a miss: another version of the app saved it.
//! Replacement is atomic per file. The last-key pointer is written only after
//! the complete catalog; a pointer-write failure leaves the previous pointer.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::definitions::entries;
use binary_reader::{Endian, Reader};
use eg_trainer_wire::{definitions, locale::GameLocale, protocol::ProtocolError};
use file_ops::{atomic, hashes};

use crate::definitions::catalog::{CatalogIdentity, GameCatalog, MAX_CATALOG_BYTES};

pub const KEPT_BUILDS: usize = 3;
pub const POINTER_FILE: &str = "last-key.txt";
pub const CATALOG_FILE: &str = "catalog.bin";
const MAGIC: &[u8; 8] = b"EGCAT001";
/// 2: the entries as four whole lists (1 kept them in pages of 16).
const SCHEMA_VERSION: u32 = 2;
const CHECKSUM_BYTES: usize = 64;

#[derive(Debug)]
pub enum CacheError {
    InvalidData,
    IdentityMismatch,
    TooLarge,
    Io(io::Error),
}
impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData => f.write_str("invalid catalog cache"),
            Self::IdentityMismatch => f.write_str("catalog cache identity mismatch"),
            Self::TooLarge => f.write_str("catalog cache exceeds its size limit"),
            Self::Io(error) => write!(f, "catalog cache I/O: {error}"),
        }
    }
}
impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
impl From<io::Error> for CacheError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<ProtocolError> for CacheError {
    fn from(_: ProtocolError) -> Self {
        Self::InvalidData
    }
}
impl From<binary_reader::Error> for CacheError {
    fn from(_: binary_reader::Error) -> Self {
        Self::InvalidData
    }
}

pub struct CatalogCache {
    directory: PathBuf,
}
impl CatalogCache {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self { directory: directory.into() }
    }

    pub fn path_for(&self, identity: &CatalogIdentity) -> Result<PathBuf, CacheError> {
        Ok(self.directory.join(identity_key(identity)?).join(CATALOG_FILE))
    }

    /// Absence, and a file of another schema, are misses. Corruption, another identity
    /// and I/O errors are explicit, so callers can retain the damaged file and report a fault.
    pub fn load(&self, identity: &CatalogIdentity) -> Result<Option<GameCatalog>, CacheError> {
        let key = identity_key(identity)?;
        let Some(bytes) = read_bounded(&self.directory.join(key).join(CATALOG_FILE))? else {
            return Ok(None);
        };
        let Some(catalog) = decode(&bytes)? else {
            return Ok(None);
        };
        if &catalog.identity != identity {
            return Err(CacheError::IdentityMismatch);
        }
        Ok(Some(catalog))
    }

    pub fn load_last(&self) -> Result<Option<GameCatalog>, CacheError> {
        let Some(bytes) = file_ops::read::if_present(file_ops::read::head(&self.directory.join(POINTER_FILE), 65))? else {
            return Ok(None);
        };
        let key = std::str::from_utf8(&bytes).map_err(|_| CacheError::InvalidData)?;
        if !hashes::is_sha256(key) {
            return Err(CacheError::InvalidData);
        }
        let Some(bytes) = read_bounded(&self.directory.join(key).join(CATALOG_FILE))? else {
            return Ok(None);
        };
        let Some(catalog) = decode(&bytes)? else {
            return Ok(None);
        };
        if identity_key(&catalog.identity)? != key {
            return Err(CacheError::IdentityMismatch);
        }
        Ok(Some(catalog))
    }

    /// Saves a fully validated catalog, then the last-key pointer. Retention is
    /// best effort: cleanup failure costs disk space.
    pub fn save(&self, catalog: &GameCatalog) -> Result<(), CacheError> {
        let key = identity_key(&catalog.identity)?;
        let bytes = encode(catalog)?;
        atomic::write(&self.directory.join(&key).join(CATALOG_FILE), &bytes)?;
        atomic::write(&self.directory.join(POINTER_FILE), key.as_bytes())?;
        self.prune(&key);
        Ok(())
    }

    fn prune(&self, current: &str) {
        let Ok(entries) = fs::read_dir(&self.directory) else { return };
        let mut previous: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                if name == current || !hashes::is_sha256(&name) || !entry.file_type().ok()?.is_dir() {
                    return None;
                }
                let file = entry.path().join(CATALOG_FILE);
                let metadata = fs::symlink_metadata(&file).ok()?;
                if !metadata.file_type().is_file() {
                    return None;
                }
                Some((metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), name, entry.path()))
            })
            .collect();
        previous.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, _, directory) in previous.into_iter().skip(KEPT_BUILDS - 1) {
            // Only this cache's file is owned here; unrelated files are retained.
            if fs::remove_file(directory.join(CATALOG_FILE)).is_ok() {
                let _ = fs::remove_dir(directory);
            }
        }
    }
}

pub fn identity_key(identity: &CatalogIdentity) -> Result<String, CacheError> {
    identity.check()?;
    let mut bytes = b"EG catalog identity v1\0".to_vec();
    write_identity(&mut bytes, identity)?;
    Ok(hashes::of_bytes(&bytes))
}

fn append(bytes: &mut Vec<u8>, more: &[u8]) -> Result<(), CacheError> {
    if bytes.len().checked_add(more.len()).is_none_or(|n| n > MAX_CATALOG_BYTES - CHECKSUM_BYTES) {
        return Err(CacheError::TooLarge);
    }
    bytes.extend_from_slice(more);
    Ok(())
}
fn number(bytes: &mut Vec<u8>, n: u32) -> Result<(), CacheError> {
    append(bytes, &n.to_le_bytes())
}
fn text(bytes: &mut Vec<u8>, s: &str) -> Result<(), CacheError> {
    number(bytes, s.len() as u32)?;
    append(bytes, s.as_bytes())
}
fn write_identity(bytes: &mut Vec<u8>, identity: &CatalogIdentity) -> Result<(), CacheError> {
    text(bytes, &identity.binary_hash)?;
    text(bytes, &identity.metadata_hash)?;
    text(bytes, &identity.locale.language)?;
    text(bytes, &identity.locale.game_version)?;
    number(bytes, identity.extraction_version)
}

fn encode(catalog: &GameCatalog) -> Result<Vec<u8>, CacheError> {
    // Writing the entries checks them; the rest is checked here.
    catalog.check_keys()?;
    let mut bytes = Vec::new();
    append(&mut bytes, MAGIC)?;
    number(&mut bytes, SCHEMA_VERSION)?;
    write_identity(&mut bytes, &catalog.identity)?;
    append(&mut bytes, &definitions::encode(&catalog.limits)?)?;
    append(&mut bytes, &entries::encode(catalog)?)?;
    let checksum = hashes::of_bytes(&bytes);
    bytes.extend_from_slice(checksum.as_bytes());
    Ok(bytes)
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, CacheError> {
    file_ops::read::if_present(file_ops::read::bounded(path, MAX_CATALOG_BYTES as u64)).map_err(|e| match e.kind() {
        io::ErrorKind::FileTooLarge => CacheError::TooLarge,
        _ => e.into(),
    })
}

fn read_text(r: &mut Reader, cap: usize) -> Result<String, CacheError> {
    let len = r.u32()? as usize;
    if len > cap {
        return Err(CacheError::InvalidData);
    }
    std::str::from_utf8(r.take(len)?).map(str::to_owned).map_err(|_| CacheError::InvalidData)
}
/// The catalog a file holds, or none when another schema wrote it.
fn decode(bytes: &[u8]) -> Result<Option<GameCatalog>, CacheError> {
    let split = bytes.len().checked_sub(CHECKSUM_BYTES).ok_or(CacheError::InvalidData)?;
    if hashes::of_bytes(&bytes[..split]).as_bytes() != &bytes[split..] {
        return Err(CacheError::InvalidData);
    }
    let mut r = Reader::new(&bytes[..split], Endian::Little);
    if r.take(8)? != MAGIC {
        return Err(CacheError::InvalidData);
    }
    if r.u32()? != SCHEMA_VERSION {
        return Ok(None);
    }
    let identity = CatalogIdentity {
        binary_hash: read_text(&mut r, 64)?,
        metadata_hash: read_text(&mut r, 64)?,
        locale: GameLocale {
            language: read_text(&mut r, entries::MAX_TEXT_BYTES)?,
            game_version: read_text(&mut r, entries::MAX_TEXT_BYTES)?,
        },
        extraction_version: r.u32()?,
    };
    let limits = definitions::decode(r.take(40)?)?;
    let entries = entries::decode(r.take(r.remaining())?)?;
    Ok(Some(GameCatalog::new(identity, limits, entries)?))
}
