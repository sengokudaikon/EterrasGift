use eg_core::game_log::{GameLogState, GameLogStatus, GameReadiness};
use eg_game_access::{bootstrap::HelperArtifact, process::ProcessIdentity};
use eg_trainer::{
    backend::{BackendError, ErrorKind},
    connection::{AdmissionCheck, LogFactory},
    gate::LogSource,
    profile::BuildProfile,
};
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

pub const SOURCE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
fn set16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn set32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn image() -> Vec<u8> {
    // Authored inert PE32+: parsed as bytes only; never loaded or executed.
    let mut bytes = vec![0; 0xc00];
    bytes[..2].copy_from_slice(b"MZ");
    set32(&mut bytes, 0x3c, 0x80);
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    set16(&mut bytes, 0x84, 0x8664);
    set16(&mut bytes, 0x86, 1);
    set16(&mut bytes, 0x94, 240);
    set16(&mut bytes, 0x96, 0x2000);
    set16(&mut bytes, 0x98, 0x20b);
    set32(&mut bytes, 0xd0, 0x2000);
    set32(&mut bytes, 0x104, 16);
    set32(&mut bytes, 0x108, 0x1100);
    set32(&mut bytes, 0x10c, 0x180);
    set32(&mut bytes, 0x188 + 8, 0x800);
    set32(&mut bytes, 0x188 + 12, 0x1000);
    set32(&mut bytes, 0x188 + 16, 0x800);
    set32(&mut bytes, 0x188 + 20, 0x400);
    set32(&mut bytes, 0x188 + 36, 0x60000020);
    set32(&mut bytes, 0x500 + 20, 2);
    set32(&mut bytes, 0x500 + 24, 2);
    set32(&mut bytes, 0x500 + 28, 0x1140);
    set32(&mut bytes, 0x500 + 32, 0x1150);
    set32(&mut bytes, 0x500 + 36, 0x1160);
    set32(&mut bytes, 0x540, 0x1000);
    set32(&mut bytes, 0x544, 0x1020);
    set32(&mut bytes, 0x550, 0x1170);
    set32(&mut bytes, 0x554, 0x1190);
    set16(&mut bytes, 0x560, 0);
    set16(&mut bytes, 0x562, 1);
    bytes[0x570..0x584].copy_from_slice(b"eg_helper_bootstrap\0");
    bytes[0x590..0x5a3].copy_from_slice(b"eg_helper_build_id\0");
    bytes[0x400..0x410].fill(0xc3);
    bytes[0x420..0x423].copy_from_slice(&[0x48, 0x8d, 0x05]);
    set32(&mut bytes, 0x423, 0x1400 - (0x1020 + 7));
    bytes[0x427] = 0xc3;
    bytes[0x800..0x840].copy_from_slice(SOURCE.as_bytes());
    bytes
}
pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub target: ProcessIdentity,
    pub profile: BuildProfile,
    pub artifact: Arc<HelperArtifact>,
}
impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let bytes = image();
        let binary = temp.path().join("GameAssembly.dll");
        std::fs::write(&binary, &bytes).unwrap();
        let metadata = temp.path().join("Authored_Data/il2cpp_data/Metadata/global-metadata.dat");
        std::fs::create_dir_all(metadata.parent().unwrap()).unwrap();
        std::fs::write(&metadata, b"authored metadata").unwrap();
        let executable = temp.path().join("Authored.exe");
        std::fs::write(&executable, b"never executed").unwrap();
        let helper = temp.path().join("eg_trainer_helper.dll");
        std::fs::write(&helper, &bytes).unwrap();
        let artifact = Arc::new(HelperArtifact::capture(&helper, &eg_core::hashes::of_bytes(&bytes), SOURCE).unwrap());
        let target = ProcessIdentity {
            pid: 42,
            created: 134053344000000000,
            executable,
        };
        let profile = BuildProfile::capture(&binary, &metadata, SOURCE).unwrap();
        Self { temp, target, profile, artifact }
    }
    pub fn profile_with_source(&self, source: &str) -> BuildProfile {
        BuildProfile::capture(
            &self.temp.path().join("GameAssembly.dll"),
            &self.temp.path().join("Authored_Data/il2cpp_data/Metadata/global-metadata.dat"),
            source,
        )
        .unwrap()
    }
}
impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}
pub fn run(future: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { tokio::time::timeout(Duration::from_secs(3), future).await.unwrap() });
}
pub struct Admission {
    pub checks: Arc<AtomicUsize>,
    pub allow: bool,
}
impl Admission {
    pub fn new(allow: bool) -> Self {
        Self {
            checks: Arc::new(AtomicUsize::new(0)),
            allow,
        }
    }
}
impl AdmissionCheck for Admission {
    fn check(&mut self) -> Result<(), BackendError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.allow {
            Ok(())
        } else {
            Err(BackendError::blocked(ErrorKind::Gate, "authored gate refusal"))
        }
    }
}
#[derive(Clone, Default)]
pub struct Logs {
    pub calls: Arc<AtomicUsize>,
}
pub struct Log;
impl LogSource for Log {
    fn read(&mut self, _: jiff::Timestamp) -> GameLogState {
        GameLogState {
            status: GameLogStatus::Current,
            readiness: GameReadiness::Offline,
            character: None,
            shell_state: Some("None".into()),
            offline_mode: Some(true),
            problem: None,
        }
    }
}
impl LogFactory for Logs {
    type Log = Log;
    fn create(&mut self, _: &ProcessIdentity) -> Result<Self::Log, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Log)
    }
}
