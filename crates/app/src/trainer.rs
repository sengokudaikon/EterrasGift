//! One background runtime owns the trainer. The window holds only its queue/watch handle.
use eg_core::fault::FaultLog;
use eg_trainer::{
    backend::{self, BackendHandle, BackendOptions, Command, SubmitError, SystemClock},
    catalog_cache::CatalogCache,
    sources::FileLogFactory,
};
use eg_trainer_host::desktop::{DesktopConnector, GameProcesses};
use std::{
    cell::Cell,
    path::PathBuf,
    thread::{self, JoinHandle},
    time::Duration,
};

thread_local! {
    static WORKER_CATCHES_PANIC: Cell<bool> = const { Cell::new(false) };
}

pub fn catches_panic() -> bool {
    WORKER_CATCHES_PANIC.get()
}

pub struct Trainer {
    pub handle: BackendHandle,
    worker: JoinHandle<bool>,
}
impl Trainer {
    pub fn start(
        helper_manifest: PathBuf,
        player_log: PathBuf,
        cache: PathBuf,
        executable_name: String,
        options: BackendOptions,
        no_connect: bool,
        faults: FaultLog,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        let logs = FileLogFactory::new(player_log).map_err(|e| e.to_string())?;
        let (handle, worker) = backend::service(
            GameProcesses::new(executable_name),
            DesktopConnector::new(helper_manifest, logs.clone()),
            logs.reader(),
            SystemClock,
            options,
            Some(CatalogCache::new(cache)),
        );
        let worker = worker.with_fault_log(faults.clone());
        let worker = thread::Builder::new()
            .name("trainer".into())
            .spawn(move || {
                run_worker(
                    || runtime.block_on(worker.run()),
                    &faults,
                    || {
                        let _ = slint::invoke_from_event_loop(|| {
                            let _ = slint::quit_event_loop();
                        });
                    },
                )
            })
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
        self.worker.join().is_ok_and(|completed| completed)
    }
}

/// Unexpected worker panics have the same fatal policy as Mods/Settings. Expected
/// connection and write failures are typed results and never enter this path.
fn run_worker(work: impl FnOnce(), faults: &FaultLog, shutdown: impl FnOnce()) -> bool {
    let previous = WORKER_CATCHES_PANIC.replace(true);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
    WORKER_CATCHES_PANIC.set(previous);
    match result {
        Ok(()) => true,
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("Unknown panic payload");
            faults.write("Trainer worker", Some(message), None, Some("Unexpected worker failure; the app closes with exit code 1."));
            shutdown();
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_panic_records_its_fault_and_requests_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let faults = FaultLog::with_fallbacks(directory.path(), Some(directory.path()), vec![]);
        let mut shutdown = false;
        assert!(!run_worker(
            || {
                assert!(catches_panic());
                panic!("fixture worker fault")
            },
            &faults,
            || shutdown = true
        ));
        assert!(!catches_panic());
        assert!(shutdown);
        let reports = std::fs::read_dir(directory.path()).unwrap().collect::<Vec<_>>();
        assert_eq!(reports.len(), 1);
        let text = std::fs::read_to_string(reports[0].as_ref().unwrap().path()).unwrap();
        assert!(text.contains("Trainer worker"));
        assert!(text.contains("fixture worker fault"));
        assert!(run_worker(|| {}, &faults, || panic!("normal completion must not close the app")));
    }
}
