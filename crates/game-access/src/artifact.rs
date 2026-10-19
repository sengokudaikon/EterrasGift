//! Read-only local Windows artifact leases. File handles are private and opened
//! with FILE_SHARE_READ, denying concurrent write/delete handles, including an
//! existing writable mapping on the tested Windows/NTFS contract. A lease pins
//! the opened file object, not its parent namespace. verify_path explicitly
//! checks the current path against its retained 128-bit file ID and volume ID.
//! No process is opened. Unsupported identities/remote paths fail closed.

#![cfg(windows)]

use eg_core::hashes;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::windows::{
        ffi::OsStringExt,
        fs::{FileExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, Path, PathBuf, Prefix},
    sync::atomic::{AtomicU64, Ordering},
};
use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetDriveTypeW, GetFileInformationByHandleEx, GetFinalPathNameByHandleW};

pub const MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;
const FILE_SHARE_READ: u32 = 1;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOVABLE: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    volume: u64,
    file: [u8; 16],
}

pub struct ReadLease {
    file: File,
    path: PathBuf,
    identity: Identity,
    length: u64,
    bytes_read: AtomicU64,
}
impl ReadLease {
    /// Capture and hash the same retained read-only file object. `path` must be
    /// canonical; following a changed reparse target is rejected before hashing.
    pub fn capture(path: &Path, expected_sha256: &str) -> io::Result<Self> {
        if !hashes::is_sha256(expected_sha256) || !path.is_absolute() || fs::canonicalize(path)? != path {
            return Err(invalid("artifact path or expected hash"));
        }
        local_disk(path)?;
        let mut file = open(path)?;
        if opened_path(&file)? != path {
            return Err(invalid("artifact path changed while opening"));
        }
        let identity = identity(&file)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("artifact is not a regular file"));
        }
        if metadata.len() > MAX_ARTIFACT_BYTES {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "artifact exceeds streaming lease limit"));
        }
        let length = metadata.len();
        let mut counted = Counted { reader: &mut file, bytes: 0 };
        let digest = hashes::of_reader((&mut counted).take(MAX_ARTIFACT_BYTES + 1))?;
        if counted.bytes != length || !digest.eq_ignore_ascii_case(expected_sha256) {
            return Err(invalid("artifact content changed"));
        }
        let bytes_read = AtomicU64::new(counted.bytes);
        let lease = Self {
            file,
            path: path.to_owned(),
            identity,
            length,
            bytes_read,
        };
        lease.verify_path()?;
        Ok(lease)
    }
    /// Metadata-only object-identity check. This does not rehash file contents or
    /// claim that an external actor cannot rename an ancestor between checks.
    pub fn verify_path(&self) -> io::Result<()> {
        let current = open(&self.path)?;
        if opened_path(&current)? != self.path || identity(&current)? != self.identity || identity(&self.file)? != self.identity {
            return Err(invalid("artifact path no longer identifies retained object"));
        }
        Ok(())
    }
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    /// Actual content bytes read by hashing and read_exact_at; path checks do not
    /// add to this counter. The counter grants no access to the retained handle.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }
    /// Bounded positional reads avoid a shared seek cursor. The handle cannot be
    /// obtained, cloned, converted to writable access, or exposed through this API.
    pub fn read_exact_at(&self, mut offset: u64, mut output: &mut [u8]) -> io::Result<()> {
        if offset.checked_add(output.len() as u64).is_none_or(|end| end > self.length) {
            return Err(invalid("artifact read bounds"));
        }
        while !output.is_empty() {
            let count = match self.file.seek_read(output, offset) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            self.bytes_read.fetch_add(count as u64, Ordering::Relaxed);
            offset += count as u64;
            output = &mut output[count..];
        }
        Ok(())
    }
}
fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path)
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn identity(file: &File) -> io::Result<Identity> {
    let mut information = FILE_ID_INFO::default();
    // SAFETY: live owned handle; output is a writable FILE_ID_INFO with its exact
    // declared size. The API is synchronous and retains no Rust pointer.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut information as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        return Err(if matches!(error.raw_os_error(), Some(1 | 50 | 87)) {
            io::Error::new(io::ErrorKind::Unsupported, error)
        } else {
            error
        });
    }
    Ok(Identity {
        volume: information.VolumeSerialNumber,
        file: information.FileId.Identifier,
    })
}
fn opened_path(file: &File) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: live owned handle and correctly sized writable UTF-16 output.
    let count = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), buffer.as_mut_ptr(), buffer.len() as u32, 0) };
    if count == 0 {
        return Err(io::Error::last_os_error());
    }
    if count as usize >= buffer.len() {
        return Err(invalid("artifact final path length"));
    }
    Ok(std::ffi::OsString::from_wide(&buffer[..count as usize]).into())
}
fn local_disk(path: &Path) -> io::Result<()> {
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return Err(invalid("artifact drive path"));
    };
    let disk = match prefix.kind() {
        Prefix::Disk(disk) | Prefix::VerbatimDisk(disk) => disk,
        _ => return Err(io::Error::new(io::ErrorKind::Unsupported, "remote artifact path")),
    };
    let root = [u16::from(disk), u16::from(b':'), u16::from(b'\\'), 0];
    // SAFETY: fixed NUL-terminated drive root, read only for the duration of call.
    let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
    if !matches!(kind, DRIVE_FIXED | DRIVE_REMOVABLE) {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "artifact drive is not local storage"));
    }
    Ok(())
}
struct Counted<R> {
    reader: R,
    bytes: u64,
}
impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.reader.read(buffer)?;
        self.bytes += count as u64;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_sparse_file_is_an_unsupported_optimization_without_reading_its_contents() {
        use windows_sys::Win32::System::IO::DeviceIoControl;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("eg-artifact-sparse-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("owned.bin");
        fs::write(&path, []).unwrap();
        let canonical = fs::canonicalize(&path).unwrap();
        // Prove this volume supports leases before testing the size-specific fallback.
        drop(ReadLease::capture(&canonical, &hashes::of_bytes(&[])).unwrap());
        let file = OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let mut returned = 0;
        // SAFETY: owned writable file handle, synchronous FSCTL_SET_SPARSE with
        // null input/output buffers, live byte-count output, and no OVERLAPPED.
        assert_ne!(
            unsafe {
                DeviceIoControl(
                    file.as_raw_handle(),
                    0x0009_00c4,
                    std::ptr::null(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        file.set_len(MAX_ARTIFACT_BYTES + 1).unwrap();
        drop(file);
        let error = ReadLease::capture(&canonical, &"0".repeat(64)).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(error.to_string(), "artifact exceeds streaming lease limit");
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_ARTIFACT_BYTES + 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
