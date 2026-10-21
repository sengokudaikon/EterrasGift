//! Local artifact identity and session leases for the selected game installation.
use eg_core::hashes;
use eg_trainer_wire::protocol::DispatcherSpec;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

#[derive(Clone, Debug)]
pub struct BuildProfile {
    binary: PathBuf,
    metadata: PathBuf,
    binary_hash: String,
    metadata_hash: String,
    helper_build_id: String,
    files: Weak<ConnectionFiles>,
}

impl PartialEq for BuildProfile {
    fn eq(&self, other: &Self) -> bool {
        self.binary == other.binary
            && self.metadata == other.metadata
            && self.binary_hash == other.binary_hash
            && self.metadata_hash == other.metadata_hash
            && self.helper_build_id == other.helper_build_id
    }
}
impl Eq for BuildProfile {}
#[derive(Debug)]
pub(crate) struct ConnectionFiles {
    #[cfg(windows)]
    binary: Arc<eg_game_access::artifact::ReadLease>,
    #[cfg(windows)]
    metadata: Arc<eg_game_access::artifact::ReadLease>,
}

/// Session-scoped validation authority. Construction is checked; fields and
/// retained handles are private. A cloned equal profile may use the same lease.
/// Windows pins read-only objects; other platforms rehash on every validation.
pub struct ProfileLease {
    profile: BuildProfile,
    dispatcher: DispatcherSpec,
    storage: LeaseStorage,
}
enum LeaseStorage {
    #[cfg(windows)]
    Held {
        binary: Arc<eg_game_access::artifact::ReadLease>,
        metadata: Arc<eg_game_access::artifact::ReadLease>,
    },
    Rehash,
}
impl ProfileLease {
    pub fn bytes_read(&self) -> Option<u64> {
        match &self.storage {
            #[cfg(windows)]
            LeaseStorage::Held { binary, metadata } => Some(binary.bytes_read() + metadata.bytes_read()),
            LeaseStorage::Rehash => None,
        }
    }
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
            files: Weak::new(),
        })
    }
    pub(crate) fn capture_connection(binary: &Path, metadata: &Path, helper_build_id: &str) -> io::Result<(Self, Option<Arc<ConnectionFiles>>)> {
        #[cfg(windows)]
        {
            if !hashes::is_sha256(helper_build_id) {
                return Err(invalid("invalid expected helper source build ID"));
            }
            let binary = fs::canonicalize(binary)?;
            let metadata = fs::canonicalize(metadata)?;
            let opened = (|| -> io::Result<_> {
                Ok(Arc::new(ConnectionFiles {
                    binary: Arc::new(eg_game_access::artifact::ReadLease::open(&binary)?),
                    metadata: Arc::new(eg_game_access::artifact::ReadLease::open(&metadata)?),
                }))
            })();
            match opened {
                Ok(files) => {
                    let profile = Self {
                        binary,
                        metadata,
                        binary_hash: files.binary.sha256().into(),
                        metadata_hash: files.metadata.sha256().into(),
                        helper_build_id: helper_build_id.to_ascii_uppercase(),
                        files: Arc::downgrade(&files),
                    };
                    return Ok((profile, Some(files)));
                }
                Err(error) if error.kind() == io::ErrorKind::Unsupported || matches!(error.raw_os_error(), Some(32 | 33)) => {}
                Err(error) => return Err(error),
            }
        }
        Self::capture(binary, metadata, helper_build_id).map(|profile| (profile, None))
    }
    #[cfg(windows)]
    pub fn binary_lease(&self) -> Option<Arc<eg_game_access::artifact::ReadLease>> {
        self.files.upgrade().map(|files| files.binary.clone())
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

    pub fn capture_lease(&self, dispatcher: &DispatcherSpec) -> io::Result<ProfileLease> {
        #[cfg(windows)]
        let storage = match (|| -> io::Result<LeaseStorage> {
            let (binary, metadata) = match self.files.upgrade() {
                Some(files) => (files.binary.clone(), files.metadata.clone()),
                None => (
                    Arc::new(eg_game_access::artifact::ReadLease::capture(&self.binary, &self.binary_hash)?),
                    Arc::new(eg_game_access::artifact::ReadLease::capture(&self.metadata, &self.metadata_hash)?),
                ),
            };
            validate_dispatcher_lease(&binary, dispatcher)?;
            Ok(LeaseStorage::Held { binary, metadata })
        })() {
            Ok(storage) => storage,
            // A capability/share-mode limitation is not proof that an artifact
            // changed. Preserve the existing full-per-read validation fallback.
            Err(error) if error.kind() == io::ErrorKind::Unsupported || matches!(error.raw_os_error(), Some(32 | 33)) => {
                self.validate(dispatcher)?;
                LeaseStorage::Rehash
            }
            Err(error) => return Err(error),
        };
        #[cfg(not(windows))]
        let storage = {
            self.validate(dispatcher)?;
            LeaseStorage::Rehash
        };
        let lease = ProfileLease {
            profile: self.clone(),
            dispatcher: *dispatcher,
            storage,
        };
        self.validate_lease(dispatcher, &lease)?;
        Ok(lease)
    }
    pub(crate) fn validate_lease(&self, dispatcher: &DispatcherSpec, lease: &ProfileLease) -> io::Result<()> {
        if &lease.profile != self || &lease.dispatcher != dispatcher {
            return Err(invalid("artifact lease belongs to another profile or dispatcher"));
        }
        match &lease.storage {
            #[cfg(windows)]
            LeaseStorage::Held { binary, metadata } => {
                binary.verify_path()?;
                metadata.verify_path()?;
                Ok(())
            }
            LeaseStorage::Rehash => self.validate(dispatcher),
        }
    }
}

#[cfg(windows)]
fn validate_dispatcher_lease(image: &eg_game_access::artifact::ReadLease, dispatcher: &DispatcherSpec) -> io::Result<()> {
    let layout = eg_game_access::pe::Layout::read(image.len(), |at, output| image.read_exact_at(at, output))?;
    let at = layout.file_offset(dispatcher.rva, dispatcher.bytes.len(), true)?;
    let mut bytes = [0; 16];
    image.read_exact_at(at, &mut bytes)?;
    if bytes != dispatcher.bytes {
        return Err(invalid("dispatcher differs from executable game file"));
    }
    Ok(())
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn validate_dispatcher(image: &[u8], dispatcher: &DispatcherSpec) -> io::Result<()> {
    let layout = eg_game_access::pe::Layout::from_bytes(image)?;
    let at = layout.file_offset(dispatcher.rva, dispatcher.bytes.len(), true)? as usize;
    if image.get(at..at + dispatcher.bytes.len()) != Some(dispatcher.bytes.as_slice()) {
        return Err(invalid("dispatcher differs from executable game file"));
    }
    Ok(())
}
