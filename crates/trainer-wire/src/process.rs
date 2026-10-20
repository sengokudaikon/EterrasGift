//! Owned process identity is data. Capturing or querying a process belongs to the host.
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Windows FILETIME: 100 ns since 1601. PID reuse is a different identity.
    pub created: u64,
    pub executable: PathBuf,
}
