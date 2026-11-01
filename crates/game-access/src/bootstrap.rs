//! Explicit helper artifact/configuration validation. Nothing is loaded by construction.
//!
//! This module's pure boundary rejects incompatible DLLs and bootstrap replies before they
//! can authorize a process operation. A source ID is compatibility/provenance, not authentication.
//! Process admission and retained-code lifetime remain separate from parsing these bytes.

use file_ops::hashes;
#[cfg(windows)]
pub mod windows;
#[cfg(not(windows))]
use std::{fs::File, io::Read};
use std::{
    io,
    path::{Path, PathBuf},
};

pub const CONFIG_SIZE: usize = eg_trainer_wire::bootstrap::SIZE;
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

/// Initialized wire storage (`eg_trainer_wire::bootstrap`, the helper's own layout), retained until the target
/// thread's termination is proved.
#[derive(Clone, Debug)]
pub struct BootstrapConfig {
    config: eg_trainer_wire::bootstrap::BootstrapConfig,
    bytes: [u8; CONFIG_SIZE],
}
impl BootstrapConfig {
    pub fn new(pid: u32, build_id: &str) -> Result<Self> {
        if pid == 0 || !eg_trainer_wire::protocol::is_build_id(build_id) {
            return invalid("PID or build ID");
        }
        let build_id = build_id.as_bytes().try_into().or_else(|_| invalid("build ID length"))?;
        Ok(Self::of(eg_trainer_wire::bootstrap::BootstrapConfig::new(pid, build_id)))
    }
    fn of(config: eg_trainer_wire::bootstrap::BootstrapConfig) -> Self {
        Self { config, bytes: config.to_bytes() }
    }
    /// Bind the owning controller before the first pipe exchange, so its death
    /// also stops a helper whose Discover handshake never completed.
    pub fn with_owner(self, pid: u32, created: u64) -> Result<Self> {
        if pid == 0 || created == 0 {
            return invalid("controller identity");
        }
        Ok(Self::of(eg_trainer_wire::bootstrap::BootstrapConfig {
            owner_pid: pid,
            owner_created: created,
            ..self.config
        }))
    }
    pub fn bytes(&self) -> &[u8; CONFIG_SIZE] {
        &self.bytes
    }
    pub fn decode_reply(&self, exit_code: u32, result: &[u8]) -> Result<BootstrapReply> {
        let result: &[u8; CONFIG_SIZE] = result.try_into().map_err(|_| BootstrapError::Invalid("configuration reply size"))?;
        let reply = eg_trainer_wire::bootstrap::BootstrapConfig::from_bytes(result);
        // Only the status is the helper's to write.
        if (eg_trainer_wire::bootstrap::BootstrapConfig {
            status: self.config.status,
            ..reply
        }) != self.config
        {
            return invalid("configuration immutable fields changed");
        }
        if exit_code != 0 || reply.status != 0 {
            return Err(BootstrapError::Rejected { exit_code, status: reply.status });
        }
        Ok(BootstrapReply::Started)
    }
}

/// The open file denies write/delete sharing on Windows through startup. Capture hashes and
/// validates bytes without loading the DLL into either the controller or the game process.
pub struct HelperArtifact {
    path: PathBuf,
    sha256: String,
    bootstrap_rva: u32,
    image_size: u32,
    #[cfg(not(windows))]
    file: File,
    #[cfg(windows)]
    lease: file_ops::lease::ReadLease,
    bytes: Vec<u8>,
}
impl HelperArtifact {
    pub fn capture(path: &Path, expected_sha256: &str) -> Result<Self> {
        if !path.is_absolute() || !hashes::is_sha256(expected_sha256) {
            return invalid("artifact identity");
        }
        let path = std::fs::canonicalize(path)?;
        let metadata = std::fs::metadata(&path)?;
        if !metadata.is_file() || metadata.len() > MAX_HELPER_BYTES {
            return invalid("artifact kind or size");
        }
        #[cfg(windows)]
        let lease = file_ops::lease::ReadLease::capture(&path, expected_sha256)?;
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
        let layout = HelperLayout::parse(&bytes)?;
        Ok(Self {
            path,
            sha256: expected_sha256.to_ascii_uppercase(),
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
        let layout = binary_reader::pe::Layout::from_bytes(&self.bytes)?;
        check_image_size(&layout)?;
        Ok(layout.span(&self.bytes, rva, length, true)?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelperLayout {
    pub bootstrap_rva: u32,
    pub image_size: u32,
}

impl HelperLayout {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let layout = binary_reader::pe::Layout::from_bytes(bytes)?;
        check_image_size(&layout)?;
        let bootstrap_rva = layout.export(bytes, b"eg_helper_bootstrap")?;
        layout.file_offset(u64::from(bootstrap_rva), 16, true)?;
        Ok(Self {
            bootstrap_rva,
            image_size: layout.image_size,
        })
    }
}

/// A DLL image of a size this helper's loader would map.
fn check_image_size(layout: &binary_reader::pe::Layout) -> Result<()> {
    if !layout.dll || layout.image_size == 0 || layout.image_size > 512 * 1024 * 1024 {
        return invalid("Windows x64 DLL image size/header");
    }
    Ok(())
}
