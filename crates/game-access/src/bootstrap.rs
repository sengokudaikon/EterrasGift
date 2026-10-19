//! Explicit helper artifact/configuration validation. Nothing is loaded by construction.
//!
//! This module's pure boundary rejects incompatible DLLs and bootstrap replies before they
//! can authorize a process operation. A source ID is compatibility/provenance, not authentication.
//! Process admission and retained-code lifetime remain separate from parsing these bytes.

use eg_core::hashes;
#[cfg(windows)]
pub mod windows;
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

const CONFIG_MAGIC: u32 = 0x4547_4253;
pub const CONFIG_SIZE: usize = 112;
const MAX_HELPER_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum BootstrapOperation {
    Start = 1,
    Stop = 2,
    InspectStopped = 3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootstrapReply {
    Started,
    StopAccepted { metric_addresses: [u64; 3] },
    StoppedMetrics { entry_restored: u64, server_running: u64, failure: u64 },
}

#[derive(Debug)]
pub enum BootstrapError {
    Invalid(&'static str),
    Io(io::Error),
    /// The target bootstrap returned. This does not prove that no startup side effect occurred.
    Rejected {
        exit_code: u32,
        status: i32,
    },
}
impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "invalid helper bootstrap: {reason}"),
            Self::Io(error) => write!(f, "helper bootstrap I/O: {error}"),
            Self::Rejected { exit_code, status } => write!(f, "helper bootstrap rejected: exit {exit_code}, status {status}"),
        }
    }
}
impl std::error::Error for BootstrapError {}
impl From<io::Error> for BootstrapError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
type Result<T> = std::result::Result<T, BootstrapError>;
fn invalid<T>(reason: &'static str) -> Result<T> {
    Err(BootstrapError::Invalid(reason))
}

/// Initialized wire storage, retained until the target thread's termination is proved.
#[derive(Clone, Debug)]
pub struct BootstrapConfig {
    bytes: [u8; CONFIG_SIZE],
    operation: BootstrapOperation,
}
impl BootstrapConfig {
    pub fn new(pid: u32, main_tid: u32, source_id: &str, operation: BootstrapOperation) -> Result<Self> {
        if pid == 0 || main_tid == 0 || !source_id_valid(source_id) {
            return invalid("PID, main thread or source ID");
        }
        let mut bytes = [0; CONFIG_SIZE];
        for (at, value) in [(0, CONFIG_MAGIC), (4, CONFIG_SIZE as u32), (8, pid), (12, main_tid), (16, operation as u32)] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[20..24].copy_from_slice(&(-1_i32).to_le_bytes());
        bytes[24..88].copy_from_slice(source_id.as_bytes());
        Ok(Self { bytes, operation })
    }
    pub fn bytes(&self) -> &[u8; CONFIG_SIZE] {
        &self.bytes
    }
    pub fn decode_reply(&self, exit_code: u32, result: &[u8]) -> Result<BootstrapReply> {
        let result: &[u8; CONFIG_SIZE] = result.try_into().map_err(|_| BootstrapError::Invalid("configuration reply size"))?;
        if result[..20] != self.bytes[..20] || result[24..88] != self.bytes[24..88] || (self.operation == BootstrapOperation::Start && result[88..] != self.bytes[88..]) {
            return invalid("configuration immutable fields changed");
        }
        let status = i32::from_le_bytes(result[20..24].try_into().unwrap());
        if exit_code != 0 || status != 0 {
            return Err(BootstrapError::Rejected { exit_code, status });
        }
        let metrics = std::array::from_fn(|i| u64::from_le_bytes(result[88 + i * 8..96 + i * 8].try_into().unwrap()));
        match self.operation {
            BootstrapOperation::Start => Ok(BootstrapReply::Started),
            BootstrapOperation::Stop => Ok(BootstrapReply::StopAccepted { metric_addresses: metrics }),
            BootstrapOperation::InspectStopped => Ok(BootstrapReply::StoppedMetrics {
                entry_restored: metrics[0],
                server_running: metrics[1],
                failure: metrics[2],
            }),
        }
    }
}

pub fn source_id_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The stop reply supplies retained u64 atomic addresses inside the exact loaded helper.
/// Return offsets only after validating the complete address set; no dereference occurs here.
pub fn metric_offsets(base: u64, image_size: u32, addresses: [u64; 3]) -> Result<[u32; 3]> {
    let end = base.checked_add(u64::from(image_size)).ok_or(BootstrapError::Invalid("telemetry image overflow"))?;
    if base == 0
        || image_size < 8
        || addresses[0] == addresses[1]
        || addresses[0] == addresses[2]
        || addresses[1] == addresses[2]
        || addresses.iter().any(|a| a % 8 != 0 || *a < base || a.checked_add(8).is_none_or(|last| last > end))
    {
        return invalid("telemetry address set");
    }
    Ok(addresses.map(|address| (address - base) as u32))
}

/// The open file denies write/delete sharing on Windows through startup. Capture hashes and
/// validates bytes without loading the DLL into either the controller or the game process.
pub struct HelperArtifact {
    path: PathBuf,
    sha256: String,
    source_id: String,
    bootstrap_rva: u32,
    image_size: u32,
    file: File,
    bytes: Vec<u8>,
}
impl HelperArtifact {
    pub fn capture(path: &Path, expected_sha256: &str, expected_source_id: &str) -> Result<Self> {
        if !path.is_absolute() || !hashes::is_sha256(expected_sha256) || !source_id_valid(expected_source_id) {
            return invalid("artifact identity");
        }
        let path = std::fs::canonicalize(path)?;
        let mut options = File::options();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
        }
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_HELPER_BYTES {
            return invalid("artifact kind or size");
        }
        let mut bytes = Vec::new();
        (&file).take(MAX_HELPER_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_HELPER_BYTES || !hashes::of_bytes(&bytes).eq_ignore_ascii_case(expected_sha256) {
            return invalid("artifact hash");
        }
        let layout = HelperLayout::parse(&bytes, expected_source_id)?;
        #[cfg(windows)]
        let path = opened_path(&file)?;
        Ok(Self {
            path,
            sha256: expected_sha256.to_ascii_uppercase(),
            source_id: expected_source_id.into(),
            bootstrap_rva: layout.bootstrap_rva,
            image_size: layout.image_size,
            file,
            bytes,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn bootstrap_rva(&self) -> u32 {
        self.bootstrap_rva
    }
    pub fn image_size(&self) -> u32 {
        self.image_size
    }
    /// Keep the file lease observable without exposing a mutable file cursor or bytes.
    pub fn verify_unchanged(&self) -> Result<()> {
        let mut bytes = Vec::new();
        // Reopen by path: no mutable shared seek position, and the original read-only lease stays
        // open. On Windows incompatible writers/replacement cannot coexist with that lease.
        File::open(&self.path)?.take(MAX_HELPER_BYTES + 1).read_to_end(&mut bytes)?;
        if self.file.metadata()?.len() != self.bytes.len() as u64 || bytes != self.bytes {
            return invalid("artifact changed");
        }
        Ok(())
    }
    pub fn code_bytes(&self, rva: u32, length: usize) -> Result<&[u8]> {
        Pe::parse(&self.bytes)?.at(rva, length, true)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelperLayout {
    pub bootstrap_rva: u32,
    pub image_size: u32,
}

#[cfg(windows)]
fn opened_path(file: &File) -> Result<PathBuf> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buffer = vec![0_u16; 32768];
    // SAFETY: owned live file handle and writable output with the stated unit capacity.
    let count = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), buffer.as_mut_ptr(), buffer.len() as u32, 0) };
    if count == 0 {
        return Err(io::Error::last_os_error().into());
    }
    if count as usize >= buffer.len() {
        return invalid("opened artifact path length");
    }
    Ok(std::ffi::OsString::from_wide(&buffer[..count as usize]).into())
}

impl HelperLayout {
    pub fn parse(bytes: &[u8], expected_source_id: &str) -> Result<Self> {
        if !source_id_valid(expected_source_id) {
            return invalid("expected source ID");
        }
        let pe = Pe::parse(bytes)?;
        let bootstrap_rva = pe.export(b"eg_helper_bootstrap")?;
        pe.at(bootstrap_rva, 16, true)?;
        // The evaluated getter is a leaf `lea rax,[rip+disp32]; ret`. Decode its target
        // instead of executing a candidate DLL or finding an unrelated 64-byte hex string.
        let getter_rva = pe.export(b"eg_helper_build_id")?;
        let getter = pe.at(getter_rva, 8, true)?;
        if getter[..3] != [0x48, 0x8d, 0x05] || getter[7] != 0xc3 {
            return invalid("source getter adapter");
        }
        let delta = i32::from_le_bytes(getter[3..7].try_into().unwrap());
        let source_rva = i64::from(getter_rva) + 7 + i64::from(delta);
        let source = pe.at(source_rva.try_into().map_err(|_| BootstrapError::Invalid("source getter target"))?, 65, false)?;
        if source[..64] != *expected_source_id.as_bytes() || source[64] != 0 {
            return invalid("compiled source ID");
        }
        Ok(Self {
            bootstrap_rva,
            image_size: pe.image_size,
        })
    }
}

#[derive(Clone, Copy)]
struct Section {
    va: u32,
    virtual_size: u32,
    raw_at: u32,
    raw_size: u32,
    flags: u32,
}
struct Pe<'a> {
    bytes: &'a [u8],
    image_size: u32,
    sections: Vec<Section>,
    exports: (u32, u32),
}
fn data(bytes: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    bytes
        .get(at..at.checked_add(len).ok_or(BootstrapError::Invalid("PE integer overflow"))?)
        .ok_or(BootstrapError::Invalid("PE file bounds"))
}
fn word(bytes: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(data(bytes, at, 2)?.try_into().unwrap()))
}
fn dword(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(data(bytes, at, 4)?.try_into().unwrap()))
}
impl<'a> Pe<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self> {
        if data(bytes, 0, 2)? != b"MZ" {
            return invalid("DOS header");
        }
        let nt = dword(bytes, 0x3c)? as usize;
        if data(bytes, nt, 4)? != b"PE\0\0" || word(bytes, nt + 4)? != 0x8664 || word(bytes, nt + 24)? != 0x20b || word(bytes, nt + 22)? & 0x2000 == 0 {
            return invalid("Windows x64 DLL header");
        }
        let count = word(bytes, nt + 6)?;
        let optional = word(bytes, nt + 20)? as usize;
        if !(1..=96).contains(&count) || optional < 240 || dword(bytes, nt + 24 + 108)? < 1 {
            return invalid("PE table shape");
        }
        data(bytes, nt + 24, optional)?;
        let image_size = dword(bytes, nt + 24 + 56)?;
        if image_size == 0 || image_size > 512 * 1024 * 1024 {
            return invalid("PE image size");
        }
        let exports = (dword(bytes, nt + 24 + 112)?, dword(bytes, nt + 24 + 116)?);
        if exports.0 == 0 || exports.1 < 40 || exports.0.checked_add(exports.1).is_none_or(|end| end > image_size) {
            return invalid("PE export directory bounds");
        }
        let mut sections = Vec::new();
        for i in 0..usize::from(count) {
            let at = nt + 24 + optional + i * 40;
            data(bytes, at, 40)?;
            let section = Section {
                virtual_size: dword(bytes, at + 8)?,
                va: dword(bytes, at + 12)?,
                raw_size: dword(bytes, at + 16)?,
                raw_at: dword(bytes, at + 20)?,
                flags: dword(bytes, at + 36)?,
            };
            data(bytes, section.raw_at as usize, section.raw_size as usize)?;
            if section.va.checked_add(section.virtual_size.max(section.raw_size)).is_none_or(|end| end > image_size) {
                return invalid("PE section image bounds");
            }
            let end = section.va + section.virtual_size.max(section.raw_size);
            if sections.iter().any(|other: &Section| {
                let other_end = other.va + other.virtual_size.max(other.raw_size);
                section.va < end && other.va < other_end && section.va < other_end && other.va < end
            }) {
                return invalid("PE overlapping mapped sections");
            }
            sections.push(section);
        }
        let pe = Self {
            bytes,
            image_size,
            sections,
            exports,
        };
        pe.at(exports.0, 40, false)?;
        Ok(pe)
    }
    fn at(&self, rva: u32, len: usize, executable: bool) -> Result<&'a [u8]> {
        let len = u32::try_from(len).map_err(|_| BootstrapError::Invalid("PE RVA span"))?;
        let end = rva.checked_add(len).ok_or(BootstrapError::Invalid("PE RVA overflow"))?;
        let mut result = None;
        for section in &self.sections {
            if rva >= section.va && end <= section.va.checked_add(section.raw_size).ok_or(BootstrapError::Invalid("PE raw RVA overflow"))? {
                if result.is_some() || (executable && section.flags & 0x2000_0000 == 0) {
                    return invalid("PE ambiguous/nonexecutable mapping");
                }
                let offset = (section.raw_at as usize)
                    .checked_add((rva - section.va) as usize)
                    .ok_or(BootstrapError::Invalid("PE raw offset overflow"))?;
                result = Some(data(self.bytes, offset, len as usize)?);
            }
        }
        result.ok_or(BootstrapError::Invalid("PE RVA not file-backed"))
    }
    fn name(&self, rva: u32) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        for i in 0..256 {
            let byte = self.at(rva.checked_add(i).ok_or(BootstrapError::Invalid("PE name overflow"))?, 1, false)?[0];
            if byte == 0 {
                return Ok(out);
            }
            out.push(byte);
        }
        invalid("PE export name bound")
    }
    fn export(&self, name: &[u8]) -> Result<u32> {
        let dir = self.at(self.exports.0, 40, false)?;
        let count = dword(dir, 20)?;
        let names = dword(dir, 24)?;
        if count == 0 || count > 4096 || names == 0 || names > 4096 {
            return invalid("PE export count");
        }
        let functions = self.at(dword(dir, 28)?, count as usize * 4, false)?;
        let name_rvas = self.at(dword(dir, 32)?, names as usize * 4, false)?;
        let ordinals = self.at(dword(dir, 36)?, names as usize * 2, false)?;
        let mut found = None;
        for index in 0..names as usize {
            if self.name(dword(name_rvas, index * 4)?)? == name {
                let ordinal = usize::from(word(ordinals, index * 2)?);
                if ordinal >= count as usize || found.is_some() {
                    return invalid("PE export ordinal/ambiguity");
                }
                let rva = dword(functions, ordinal * 4)?;
                if (self.exports.0..self.exports.0 + self.exports.1).contains(&rva) {
                    return invalid("forwarded helper export");
                }
                self.at(rva, 1, true)?;
                found = Some(rva);
            }
        }
        found.ok_or(BootstrapError::Invalid("required helper export missing"))
    }
}
