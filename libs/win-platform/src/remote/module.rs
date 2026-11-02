//! The modules a process has loaded, and module files held open against change.

use super::{RemoteError, Result, invalid, last, owned};
use file_ops::{hashes, lease::ReadLease};
use std::{
    ffi::OsString,
    io,
    os::windows::{ffi::OsStringExt, io::AsRawHandle},
    path::{Path, PathBuf},
    sync::Arc,
};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, GetLastError},
    System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE},
};

fn canonical(path: &Path) -> Result<PathBuf> {
    Ok(std::fs::canonicalize(path)?)
}

/// Whether two canonical paths name one file, compared without case, as Windows names files.
pub fn same_path(a: &Path, b: &Path) -> bool {
    file_ops::case::same(&a.to_string_lossy(), &b.to_string_lossy())
}

/// A canonical, hash-checked module file with a Windows write/delete-denying lease.
#[derive(Clone, Debug)]
pub struct ModuleIdentity {
    path: PathBuf,
    sha256: String,
    image_size: u32,
    lease: Arc<ReadLease>,
    layout: Arc<binary_reader::pe::Layout>,
}
impl ModuleIdentity {
    pub fn capture(path: &Path, expected_sha256: &str) -> Result<Self> {
        if !path.is_absolute() || !hashes::is_sha256(expected_sha256) {
            return invalid("module file identity");
        }
        let path = canonical(path)?;
        let lease = ReadLease::capture(&path, expected_sha256).map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                RemoteError::Invalid("module hash")
            } else {
                error.into()
            }
        })?;
        Self::from_lease(Arc::new(lease))
    }
    pub fn open(path: &Path) -> Result<Self> {
        let path = canonical(path)?;
        Self::from_lease(Arc::new(ReadLease::open(&path)?))
    }
    pub fn from_lease(lease: Arc<ReadLease>) -> Result<Self> {
        lease.verify_path()?;
        let layout = binary_reader::pe::Layout::read(lease.len(), |at, output| lease.read_exact_at(at, output))?;
        if !layout.is_mappable_dll() {
            return invalid("Windows x64 DLL image size/header");
        }
        Ok(Self {
            path: lease.path().to_owned(),
            sha256: lease.sha256().to_ascii_uppercase(),
            image_size: layout.image_size,
            lease,
            layout: Arc::new(layout),
        })
    }
    /// `length` bytes of the file at `rva`, in a section that is executable if `executable` says so.
    pub fn bytes(&self, rva: u32, length: usize, executable: bool) -> Result<Vec<u8>> {
        let at = self.layout.file_offset(u64::from(rva), length, executable)?;
        let mut bytes = vec![0; length];
        self.lease.read_exact_at(at, &mut bytes)?;
        Ok(bytes)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn image_size(&self) -> u32 {
        self.image_size
    }
    pub fn layout(&self) -> &binary_reader::pe::Layout {
        &self.layout
    }
    pub fn lease(&self) -> &ReadLease {
        &self.lease
    }
    pub fn verify_unchanged(&self) -> Result<()> {
        self.lease.verify_path()?;
        Ok(())
    }
}

/// A module loaded in a process.
#[derive(Clone, Debug)]
pub struct Module {
    pub base: u64,
    pub size: u32,
    pub path: PathBuf,
}

/// The modules loaded in process `pid` now, each with a canonical path. The caller has established that the process is
/// x64, as this one is: the snapshot would otherwise list a different set.
pub fn inventory(pid: u32) -> Result<Vec<Module>> {
    // SAFETY: read-only module metadata snapshot; x64 identity is established by preflight.
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) })?;
    // SAFETY: zero initialization is valid for the SDK module record; dwSize is set before use.
    let mut entry: MODULEENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
    let mut modules = Vec::new();
    // SAFETY: owned snapshot and initialized size-tagged output stay alive through enumeration.
    let mut more = unsafe { Module32FirstW(snapshot.as_raw_handle(), &mut entry) };
    while more != 0 {
        if modules.len() >= 4096 {
            return invalid("module inventory bound");
        }
        let len = entry.szExePath.iter().position(|c| *c == 0).ok_or(RemoteError::Invalid("module path truncation"))?;
        if len == 0 {
            return invalid("empty module path");
        }
        modules.push(Module {
            base: entry.modBaseAddr as usize as u64,
            size: entry.modBaseSize,
            path: canonical(Path::new(&OsString::from_wide(&entry.szExePath[..len])))?,
        });
        // SAFETY: same owned snapshot and initialized output as Module32FirstW.
        more = unsafe { Module32NextW(snapshot.as_raw_handle(), &mut entry) };
    }
    // SAFETY: argument-free thread-local error query immediately follows failed enumeration.
    if unsafe { GetLastError() } != ERROR_NO_MORE_FILES {
        return Err(last());
    }
    Ok(modules)
}

/// The one loaded module that is the file `identity` holds, at its image size, with no other module of its file name.
pub fn find_module<'a>(modules: &'a [Module], identity: &ModuleIdentity) -> Result<&'a Module> {
    identity.verify_unchanged()?;
    let mut found = modules.iter().filter(|m| same_path(&m.path, identity.path()));
    let item = found.next().ok_or(RemoteError::Invalid("expected target module absent"))?;
    if found.next().is_some() || item.base == 0 || item.size != identity.image_size {
        return invalid("ambiguous target module or image size");
    }
    if modules
        .iter()
        .filter(|m| {
            m.path.file_name().is_some_and(|name| {
                identity
                    .path
                    .file_name()
                    .is_some_and(|expected| file_ops::case::same(&name.to_string_lossy(), &expected.to_string_lossy()))
            })
        })
        .count()
        != 1
    {
        return invalid("multiple loaded modules have the expected basename");
    }
    Ok(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn set16(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes())
    }
    fn set32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes())
    }
    /// A small PE32+ DLL of one executable section.
    fn image() -> Vec<u8> {
        let mut bytes = vec![0; 0x1400];
        bytes[..2].copy_from_slice(b"MZ");
        set32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        set16(&mut bytes, 0x84, 0x8664);
        set16(&mut bytes, 0x86, 1);
        set16(&mut bytes, 0x94, 240);
        set16(&mut bytes, 0x96, 0x2000);
        set16(&mut bytes, 0x98, 0x20b);
        set32(&mut bytes, 0xd0, 0x3000);
        set32(&mut bytes, 0x104, 16);
        set32(&mut bytes, 0x188 + 8, 0x1000);
        set32(&mut bytes, 0x188 + 12, 0x1000);
        set32(&mut bytes, 0x188 + 16, 0x1000);
        set32(&mut bytes, 0x188 + 20, 0x400);
        set32(&mut bytes, 0x188 + 36, 0x60000020);
        bytes
    }

    #[test]
    fn module_verification_reuses_shared_lease_without_content_reads() {
        let mut bytes = image();
        bytes.resize(8 * 1024 * 1024, 0);
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("Module.dll");
        std::fs::write(&path, &bytes).unwrap();
        let module = ModuleIdentity::capture(&path, &hashes::of_bytes(&bytes)).unwrap();
        let shared = module.clone();
        let initial = module.lease().bytes_read();
        for _ in 0..20 {
            shared.verify_unchanged().unwrap();
        }
        assert_eq!(module.lease().bytes_read(), initial);
        assert!(File::options().write(true).open(&path).is_err());
        drop(module);
        assert!(File::options().write(true).open(&path).is_err());
        drop(shared);
        assert!(File::options().write(true).open(&path).is_ok());
    }

    #[test]
    fn a_file_that_is_not_the_expected_hash_says_so_by_its_reason() {
        let bytes = image();
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("Module.dll");
        std::fs::write(&path, &bytes).unwrap();
        let other = hashes::of_bytes(b"something else");
        assert!(matches!(ModuleIdentity::capture(&path, &other), Err(RemoteError::Invalid("module hash"))));
    }
}
