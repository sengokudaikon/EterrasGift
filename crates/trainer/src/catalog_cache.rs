//! Bounded, versioned definition persistence. A cache is captured data, not proof
//! that definitions still belong to a running session. No process is accessed.
//!
//! The key hashes length-delimited identity fields, including extraction version.
//! The file contains that identity, literal limits, and canonical catalog pages,
//! followed by an ASCII SHA-256 of all preceding bytes. Both envelope and pages
//! are revalidated; public GameCatalog construction does not bypass the builder.
//! Replacement is atomic per file. The last-key pointer is written only after
//! the complete catalog; a pointer-write failure leaves the previous pointer.

use std::{
    fmt,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

use eg_core::{atomic, hashes};
use eg_trainer_wire::{
    catalog::{self, CatalogEntries, CatalogKind, CatalogPage, MAX_PAGE_ENTRIES},
    definitions,
    locale::GameLocale,
    protocol::{MAX_RESPONSE_SIZE, ProtocolError},
};

use crate::catalog::{CatalogBuilder, CatalogIdentity, GameCatalog, MAX_CATALOG_BYTES};

pub const KEPT_BUILDS: usize = 3;
pub const POINTER_FILE: &str = "last-key.txt";
pub const CATALOG_FILE: &str = "catalog.bin";
const MAGIC: &[u8; 8] = b"EGCAT001";
const SCHEMA_VERSION: u32 = 1;
const MAX_PAGES: usize = 16 + 1250 + 625 + 1;
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

    /// Absence is a miss. Corruption, incompatible identity/schema, and I/O errors
    /// are explicit, so callers can retain the damaged file and report a fault.
    pub fn load(&self, identity: &CatalogIdentity) -> Result<Option<GameCatalog>, CacheError> {
        let key = identity_key(identity)?;
        let Some(bytes) = read_bounded(&self.directory.join(key).join(CATALOG_FILE))? else {
            return Ok(None);
        };
        let catalog = decode(&bytes)?;
        if &catalog.identity != identity {
            return Err(CacheError::IdentityMismatch);
        }
        Ok(Some(catalog))
    }

    pub fn load_last(&self) -> Result<Option<GameCatalog>, CacheError> {
        let pointer = match File::open(self.directory.join(POINTER_FILE)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut bytes = Vec::new();
        pointer.take(65).read_to_end(&mut bytes)?;
        let key = std::str::from_utf8(&bytes).map_err(|_| CacheError::InvalidData)?;
        if !hashes::is_sha256(key) {
            return Err(CacheError::InvalidData);
        }
        let Some(bytes) = read_bounded(&self.directory.join(key).join(CATALOG_FILE))? else {
            return Ok(None);
        };
        let catalog = decode(&bytes)?;
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
    // Reuse complete identity validation without inventing separate hash rules.
    let zero = definitions::ValueRange { minimum: 0, maximum: 0 };
    CatalogBuilder::new(
        identity.clone(),
        definitions::ItemLimits {
            forging_potential: zero,
            legendary_potential: zero,
            weavers_will: zero,
            weavers_touch: zero,
            level_requirement: zero,
        },
    )?;
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
    if catalog.bases.len() > 256
        || catalog.affixes.len() > 20_000
        || catalog.uniques.len() > 10_000
        || catalog.rarities.len() > 10
        || catalog.bases.iter().any(|(k, v)| *k != v.item_type)
        || catalog.affixes.iter().any(|(k, v)| *k != v.id)
        || catalog.uniques.iter().any(|(k, v)| *k != v.id)
        || catalog.rarities.iter().any(|(k, v)| *k != v.id)
    {
        return Err(CacheError::InvalidData);
    }
    let mut builder = CatalogBuilder::new(catalog.identity.clone(), catalog.limits)?;
    let mut bytes = Vec::new();
    append(&mut bytes, MAGIC)?;
    number(&mut bytes, SCHEMA_VERSION)?;
    write_identity(&mut bytes, &catalog.identity)?;
    append(&mut bytes, &definitions::encode(&catalog.limits)?)?;
    let page_count = [catalog.bases.len(), catalog.affixes.len(), catalog.uniques.len(), catalog.rarities.len()]
        .into_iter()
        .map(|n| n.div_ceil(MAX_PAGE_ENTRIES).max(1))
        .sum::<usize>();
    number(&mut bytes, page_count as u32)?;
    // Keep per-page allocations bounded rather than cloning the complete catalog.
    emit_pages(
        &mut bytes,
        &mut builder,
        &catalog.identity.locale,
        CatalogKind::Bases,
        catalog.bases.values(),
        CatalogEntries::Bases,
    )?;
    emit_pages(
        &mut bytes,
        &mut builder,
        &catalog.identity.locale,
        CatalogKind::Affixes,
        catalog.affixes.values(),
        CatalogEntries::Affixes,
    )?;
    emit_pages(
        &mut bytes,
        &mut builder,
        &catalog.identity.locale,
        CatalogKind::Uniques,
        catalog.uniques.values(),
        CatalogEntries::Uniques,
    )?;
    emit_pages(
        &mut bytes,
        &mut builder,
        &catalog.identity.locale,
        CatalogKind::Rarities,
        catalog.rarities.values(),
        CatalogEntries::Rarities,
    )?;
    builder.finish()?;
    let checksum = hashes::of_bytes(&bytes);
    bytes.extend_from_slice(checksum.as_bytes());
    Ok(bytes)
}

fn emit_pages<'a, T: Clone + 'a>(
    bytes: &mut Vec<u8>,
    builder: &mut CatalogBuilder,
    locale: &GameLocale,
    kind: CatalogKind,
    mut values: impl ExactSizeIterator<Item = &'a T>,
    wrap: impl Fn(Vec<T>) -> CatalogEntries,
) -> Result<(), CacheError> {
    let total = values.len() as u32;
    let mut cursor = 0;
    loop {
        let entries = wrap(values.by_ref().take(MAX_PAGE_ENTRIES).cloned().collect());
        let count = entries.len() as u32;
        let page = CatalogPage {
            locale: locale.clone(),
            kind,
            cursor,
            total,
            entries,
        };
        let payload = catalog::encode(&page)?;
        builder.push(page)?;
        number(bytes, payload.len() as u32)?;
        append(bytes, &payload)?;
        cursor += count;
        if cursor == total {
            return Ok(());
        }
    }
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, CacheError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let length = file.metadata()?.len();
    if length > MAX_CATALOG_BYTES as u64 {
        return Err(CacheError::TooLarge);
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_CATALOG_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(CacheError::TooLarge);
    }
    Ok(Some(bytes))
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn bytes(&mut self, len: usize) -> Result<&'a [u8], CacheError> {
        let (head, tail) = self.0.split_at_checked(len).ok_or(CacheError::InvalidData)?;
        self.0 = tail;
        Ok(head)
    }
    fn number(&mut self) -> Result<u32, CacheError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().map_err(|_| CacheError::InvalidData)?))
    }
    fn text(&mut self, cap: usize) -> Result<String, CacheError> {
        let len = self.number()? as usize;
        if len > cap {
            return Err(CacheError::InvalidData);
        }
        std::str::from_utf8(self.bytes(len)?).map(str::to_owned).map_err(|_| CacheError::InvalidData)
    }
}
fn decode(bytes: &[u8]) -> Result<GameCatalog, CacheError> {
    let split = bytes.len().checked_sub(CHECKSUM_BYTES).ok_or(CacheError::InvalidData)?;
    if hashes::of_bytes(&bytes[..split]).as_bytes() != &bytes[split..] {
        return Err(CacheError::InvalidData);
    }
    let mut r = Reader(&bytes[..split]);
    if r.bytes(8)? != MAGIC || r.number()? != SCHEMA_VERSION {
        return Err(CacheError::InvalidData);
    }
    let identity = CatalogIdentity {
        binary_hash: r.text(64)?,
        metadata_hash: r.text(64)?,
        locale: GameLocale {
            language: r.text(catalog::MAX_TEXT_BYTES)?,
            game_version: r.text(catalog::MAX_TEXT_BYTES)?,
        },
        extraction_version: r.number()?,
    };
    let limits = definitions::decode(r.bytes(40)?)?;
    let mut builder = CatalogBuilder::new(identity, limits)?;
    let pages = r.number()? as usize;
    if !(4..=MAX_PAGES).contains(&pages) {
        return Err(CacheError::InvalidData);
    }
    for _ in 0..pages {
        let length = r.number()? as usize;
        if length > MAX_RESPONSE_SIZE {
            return Err(CacheError::InvalidData);
        }
        builder.push(catalog::decode(r.bytes(length)?)?)?;
    }
    if !r.0.is_empty() {
        return Err(CacheError::InvalidData);
    }
    Ok(builder.finish()?)
}
