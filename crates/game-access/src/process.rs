//! Explicit process identity. Capturing it queries identity only, without memory access or injection.
#[cfg(windows)]
mod windows {
    use eg_trainer_wire::process::ProcessIdentity;
    use std::io;
    use std::os::windows::io::OwnedHandle;
    use win_platform::process::{IdentityError, identity, open_for_identity};

    pub(crate) struct Process {
        handle: OwnedHandle,
        pid: u32,
    }
    impl Process {
        pub(crate) fn open(pid: u32) -> io::Result<Self> {
            if pid == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "process ID must be nonzero"));
            }
            Ok(Self {
                handle: open_for_identity(pid)?,
                pid,
            })
        }
        pub(crate) fn integrity(&self) -> io::Result<u32> {
            win_platform::process::integrity(&self.handle)
        }
        pub(crate) fn identity(&self) -> io::Result<ProcessIdentity> {
            let identity = identity(&self.handle).map_err(|error| match error {
                IdentityError::Exited => io::Error::new(io::ErrorKind::NotFound, "target process exited"),
                IdentityError::Os(error) => error,
            })?;
            Ok(ProcessIdentity {
                pid: self.pid,
                created: identity.created,
                executable: identity.executable.into(),
            })
        }
    }
    pub fn capture(pid: u32) -> io::Result<ProcessIdentity> {
        Process::open(pid)?.identity()
    }
    /// The identity of `pid` and its mandatory integrity level (`win_platform::process::integrity`), from one handle.
    pub fn capture_with_integrity(pid: u32) -> io::Result<(ProcessIdentity, u32)> {
        let process = Process::open(pid)?;
        Ok((process.identity()?, process.integrity()?))
    }
}
#[cfg(windows)]
pub(crate) use windows::Process;

#[cfg(windows)]
pub use windows::{capture, capture_with_integrity};
