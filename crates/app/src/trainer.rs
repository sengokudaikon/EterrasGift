//! One background runtime owns the trainer. The window holds only its queue/watch handle.
use eg_trainer::{
    backend::{self, BackendHandle, BackendOptions, Command, SubmitError, SystemClock},
    catalog_cache::CatalogCache,
    sources::FileLogFactory,
};
use eg_trainer_host::desktop::{DesktopConnector, GameProcesses};
use std::{
    path::PathBuf,
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct Trainer {
    pub handle: BackendHandle,
    worker: JoinHandle<()>,
}
impl Trainer {
    pub fn start(
        helper_manifest: PathBuf,
        player_log: PathBuf,
        cache: PathBuf,
        executable_name: String,
        auto_connect: bool,
        no_connect: bool,
        override_enabled: bool,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        let logs = FileLogFactory::new(player_log).map_err(|e| e.to_string())?;
        let (handle, worker) = backend::service(
            GameProcesses::new(executable_name),
            // SAFETY: this is the supported Last Epoch desktop composition, with
            // NativePreparer's game-specific runtime and lifecycle assumptions.
            unsafe { DesktopConnector::new(helper_manifest, logs.clone()) },
            logs.reader(),
            SystemClock,
            BackendOptions {
                auto_connect,
                override_enabled,
                ..Default::default()
            },
            Some(CatalogCache::new(cache)),
        );
        let worker = thread::Builder::new()
            .name("trainer".into())
            .spawn(move || runtime.block_on(worker.run()))
            .map_err(|e| e.to_string())?;
        // No process lookup or preparation when --no-connect was requested.
        let _ = handle.try_submit(Command::Enable);
        if !no_connect {
            let _ = handle.try_submit(Command::StartMonitoring);
        }
        Ok(Self { handle, worker })
    }
    /// Called after the event loop ends. Accepted native work drains before cleanup;
    /// a full queue delays the barrier instead of silently abandoning the worker.
    pub fn close(self) -> bool {
        loop {
            match self.handle.try_submit(Command::Shutdown) {
                Ok(_) | Err(SubmitError::Stopped) => break,
                Err(SubmitError::Full) if self.worker.is_finished() => break,
                Err(SubmitError::Full) => thread::sleep(Duration::from_millis(10)),
            }
        }
        self.worker.join().is_ok()
    }
}
