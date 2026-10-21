//! Explicit helper artifact/configuration validation. Nothing is loaded by construction.
//!
//! This module's pure boundary rejects incompatible DLLs and bootstrap replies before they
//! can authorize a process operation. A source ID is compatibility/provenance, not authentication.
//! Process admission and retained-code lifetime remain separate from parsing these bytes.

use eg_core::hashes;
#[cfg(windows)]
pub mod windows;
#[cfg(not(windows))]
use std::{fs::File, io::Read};
use std::{
    io,
    path::{Path, PathBuf},
};

const CONFIG_MAGIC: u32 = 0x4547_4253;
pub const CONFIG_SIZE: usize = 80;
const MAX_HELPER_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootstrapReply {
    Started,
}

#[derive(Debug)]
pub enum BootstrapError {
    Invalid(&'static str),
    Io(io::Error),
    Admission {
        message: String,
        retry_when_waiting: bool,
    },
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
            Self::Admission { message, .. } => f.write_str(message),
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
}
impl BootstrapConfig {
    pub fn new(pid: u32, source_id: &str) -> Result<Self> {
        if pid == 0 || !source_id_valid(source_id) {
            return invalid("PID or source ID");
        }
        let mut bytes = [0; CONFIG_SIZE];
        for (at, value) in [(0, CONFIG_MAGIC), (4, CONFIG_SIZE as u32), (8, pid)] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[12..16].copy_from_slice(&(-1_i32).to_le_bytes());
        bytes[16..80].copy_from_slice(source_id.as_bytes());
        Ok(Self { bytes })
    }
    pub fn bytes(&self) -> &[u8; CONFIG_SIZE] {
        &self.bytes
    }
    pub fn decode_reply(&self, exit_code: u32, result: &[u8]) -> Result<BootstrapReply> {
        let result: &[u8; CONFIG_SIZE] = result.try_into().map_err(|_| BootstrapError::Invalid("configuration reply size"))?;
        if result[..12] != self.bytes[..12] || result[16..] != self.bytes[16..] {
            return invalid("configuration immutable fields changed");
        }
        let status = i32::from_le_bytes(result[12..16].try_into().unwrap());
        if exit_code != 0 || status != 0 {
            return Err(BootstrapError::Rejected { exit_code, status });
        }
        Ok(BootstrapReply::Started)
    }
}

pub fn source_id_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The open file denies write/delete sharing on Windows through startup. Capture hashes and
/// validates bytes without loading the DLL into either the controller or the game process.
pub struct HelperArtifact {
    path: PathBuf,
    sha256: String,
    source_id: String,
    bootstrap_rva: u32,
    image_size: u32,
    #[cfg(not(windows))]
    file: File,
    #[cfg(windows)]
    lease: crate::artifact::ReadLease,
    bytes: Vec<u8>,
}
impl HelperArtifact {
    pub fn capture(path: &Path, expected_sha256: &str, expected_source_id: &str) -> Result<Self> {
        if !path.is_absolute() || !hashes::is_sha256(expected_sha256) || !source_id_valid(expected_source_id) {
            return invalid("artifact identity");
        }
        let path = std::fs::canonicalize(path)?;
        let metadata = std::fs::metadata(&path)?;
        if !metadata.is_file() || metadata.len() > MAX_HELPER_BYTES {
            return invalid("artifact kind or size");
        }
        #[cfg(windows)]
        let lease = crate::artifact::ReadLease::capture(&path, expected_sha256)?;
        #[cfg(windows)]
        let bytes = {
            if lease.len() > MAX_HELPER_BYTES {
                return invalid("artifact kind or size");
            }
            let mut bytes = vec![0; lease.len() as usize];
            lease.read_exact_at(0, &mut bytes)?;
            bytes
        };
        #[cfg(not(windows))]
        let file = File::open(&path)?;
        #[cfg(not(windows))]
        let bytes = {
            let mut bytes = Vec::new();
            (&file).take(MAX_HELPER_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_HELPER_BYTES || !hashes::of_bytes(&bytes).eq_ignore_ascii_case(expected_sha256) {
                return invalid("artifact hash");
            }
            bytes
        };
        let layout = HelperLayout::parse(&bytes, expected_source_id)?;
        Ok(Self {
            path,
            sha256: expected_sha256.to_ascii_uppercase(),
            source_id: expected_source_id.into(),
            bootstrap_rva: layout.bootstrap_rva,
            image_size: layout.image_size,
            #[cfg(not(windows))]
            file,
            #[cfg(windows)]
            lease,
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
        #[cfg(windows)]
        {
            self.lease.verify_path()?;
        }
        #[cfg(not(windows))]
        {
            let mut bytes = Vec::new();
            // Platforms without the Windows lease must compare contents again.
            File::open(&self.path)?.take(MAX_HELPER_BYTES + 1).read_to_end(&mut bytes)?;
            if self.file.metadata()?.len() != self.bytes.len() as u64 || bytes != self.bytes {
                return invalid("artifact changed");
            }
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

impl HelperLayout {
    pub fn parse(bytes: &[u8], expected_source_id: &str) -> Result<Self> {
        if !source_id_valid(expected_source_id) {
            return invalid("expected source ID");
        }
        let pe = Pe::parse(bytes)?;
        let bootstrap_rva = pe.export(b"eg_helper_bootstrap")?;
        pe.at(bootstrap_rva, 16, true)?;
        if Self::source_id_in(&pe)? != expected_source_id {
            return invalid("compiled source ID");
        }
        Ok(Self {
            bootstrap_rva,
            image_size: pe.image_size,
        })
    }

    /// The source ID a helper DLL was compiled with, read from its image without loading it.
    pub fn compiled_source_id(bytes: &[u8]) -> Result<String> {
        Self::source_id_in(&Pe::parse(bytes)?)
    }

    fn source_id_in(pe: &Pe<'_>) -> Result<String> {
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
        let id = std::str::from_utf8(&source[..64]).ok().filter(|id| source[64] == 0 && source_id_valid(id));
        id.map(str::to_owned).ok_or(BootstrapError::Invalid("compiled source ID"))
    }
}

struct Pe<'a> {
    bytes: &'a [u8],
    image_size: u32,
    layout: crate::pe::Layout,
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
        let layout = crate::pe::Layout::from_bytes(bytes)?;
        if !layout.dll || layout.image_size == 0 || layout.image_size > 512 * 1024 * 1024 {
            return invalid("Windows x64 DLL image size/header");
        }
        let exports = layout.exports;
        if exports.0 == 0 || exports.1 < 40 || exports.0.checked_add(exports.1).is_none_or(|end| end > layout.image_size) {
            return invalid("PE export directory bounds");
        }
        let pe = Self {
            bytes,
            image_size: layout.image_size,
            layout,
            exports,
        };
        pe.at(exports.0, 40, false)?;
        Ok(pe)
    }
    fn at(&self, rva: u32, len: usize, executable: bool) -> Result<&'a [u8]> {
        let offset = self.layout.file_offset(u64::from(rva), len, executable)?;
        let offset = usize::try_from(offset).map_err(|_| BootstrapError::Invalid("PE file offset"))?;
        data(self.bytes, offset, len)
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
