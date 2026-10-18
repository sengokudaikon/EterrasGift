//! Local artifact identity, not fields transmitted by protocol21. No known-build whitelist is implied.
use eg_core::hashes;
use eg_game_access::protocol::DispatcherSpec;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct BuildProfile {
    binary: PathBuf,
    metadata: PathBuf,
    binary_hash: String,
    metadata_hash: String,
    helper_build_id: String,
}
impl BuildProfile {
    /// Files only. The process/log gate still runs before any transport/process query.
    pub fn capture(binary: &Path, metadata: &Path, helper_build_id: &str) -> io::Result<Self> {
        if !hashes::is_sha256(helper_build_id) {
            return Err(invalid("invalid expected helper source build ID"));
        }
        Ok(Self {
            binary: fs::canonicalize(binary)?,
            metadata: fs::canonicalize(metadata)?,
            binary_hash: hashes::of_bytes(&fs::read(binary)?),
            metadata_hash: hashes::of_bytes(&fs::read(metadata)?),
            helper_build_id: helper_build_id.to_ascii_uppercase(),
        })
    }
    /// Bind the two local artifacts to the selected process's actual executable directory.
    pub fn validate_target(&self, executable: &Path) -> io::Result<()> {
        let executable = fs::canonicalize(executable)?;
        let parent = executable.parent().ok_or_else(|| invalid("executable directory missing"))?;
        let mut data_name = executable.file_stem().ok_or_else(|| invalid("executable name missing"))?.to_os_string();
        data_name.push("_Data");
        let binary = fs::canonicalize(parent.join("GameAssembly.dll"))?;
        let metadata = fs::canonicalize(parent.join(data_name).join("il2cpp_data/Metadata/global-metadata.dat"))?;
        if binary != self.binary || metadata != self.metadata {
            return Err(invalid("artifact paths do not belong to selected executable"));
        }
        Ok(())
    }
    pub fn binary_hash(&self) -> &str {
        &self.binary_hash
    }
    pub fn metadata_hash(&self) -> &str {
        &self.metadata_hash
    }
    pub fn helper_build_id(&self) -> &str {
        &self.helper_build_id
    }
    /// Recheck artifact content and the file-backed executable dispatcher before the first Status.
    /// Status can install a scheduler, so discovery alone must not authorize it.
    pub fn validate(&self, dispatcher: &DispatcherSpec) -> io::Result<()> {
        let image = fs::read(&self.binary)?;
        if hashes::of_bytes(&image) != self.binary_hash || hashes::of_bytes(&fs::read(&self.metadata)?) != self.metadata_hash {
            return Err(invalid("game artifacts changed during attachment"));
        }
        validate_dispatcher(&image, dispatcher)
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn data(image: &[u8], at: usize, len: usize) -> io::Result<&[u8]> {
    image
        .get(at..at.checked_add(len).ok_or_else(|| invalid("invalid PE bounds"))?)
        .ok_or_else(|| invalid("truncated PE"))
}
fn u16_at(image: &[u8], at: usize) -> io::Result<u16> {
    Ok(u16::from_le_bytes(data(image, at, 2)?.try_into().unwrap()))
}
fn u32_at(image: &[u8], at: usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(data(image, at, 4)?.try_into().unwrap()))
}
fn validate_dispatcher(image: &[u8], dispatcher: &DispatcherSpec) -> io::Result<()> {
    if data(image, 0, 2)? != b"MZ" {
        return Err(invalid("expected DOS header"));
    }
    let pe = u32_at(image, 0x3c)? as usize;
    if data(image, pe, 4)? != b"PE\0\0" || u16_at(image, pe + 4)? != 0x8664 || u16_at(image, pe + 24)? != 0x20b {
        return Err(invalid("expected Windows x64 PE32+ game assembly"));
    }
    let optional_size = u16_at(image, pe + 20)? as usize;
    if optional_size < 112 {
        return Err(invalid("invalid PE optional header"));
    }
    let table = pe + 24 + optional_size;
    let mut matched = false;
    for index in 0..u16_at(image, pe + 6)? as usize {
        let section = table + index * 40;
        let va = u32_at(image, section + 12)? as u64;
        let raw_size = u32_at(image, section + 16)? as u64;
        let raw_at = u32_at(image, section + 20)? as usize;
        let executable = u32_at(image, section + 36)? & 0x2000_0000 != 0;
        if dispatcher.rva >= va && dispatcher.rva.checked_add(16).is_some_and(|end| end <= va + raw_size) {
            let at = raw_at.checked_add((dispatcher.rva - va) as usize).ok_or_else(|| invalid("invalid PE offset"))?;
            if matched || !executable || data(image, at, 16)? != dispatcher.bytes {
                return Err(invalid("dispatcher differs from executable game file"));
            }
            matched = true;
        }
    }
    if !matched {
        return Err(invalid("dispatcher is outside file-backed executable code"));
    }
    Ok(())
}
