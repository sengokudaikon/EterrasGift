//! Process setup and restart. The instance lock stays held until shutdown has closed every writer.

use crate::trainer;
use eg_core::fault::FaultLog;
use eg_core::paths;
use eg_core::settings::SettingsStore;
use file_ops::instance::{self, InstanceLock};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

/// The app's command line. Unknown arguments are ignored.
#[derive(Debug, Default)]
pub struct Arguments {
    /// The checkout or install the app belongs to.
    root: Option<PathBuf>,
    /// Open on Settings.
    pub settings: bool,
    /// Do not begin trainer discovery or automatic connection.
    pub no_connect: bool,
}

impl Arguments {
    /// A folder may be any OS string; the switches themselves are plain text.
    fn parse<'a>(mut arguments: impl Iterator<Item = &'a OsString>) -> Self {
        let mut parsed = Self::default();
        while let Some(argument) = arguments.next() {
            match argument.to_str() {
                Some("--root") => parsed.root = arguments.next().map(PathBuf::from),
                Some("--settings") => parsed.settings = true,
                Some("--no-connect") => parsed.no_connect = true,
                _ => {}
            }
        }
        parsed
    }
}

pub struct Startup {
    pub arguments: Arguments,
    pub root: PathBuf,
    pub settings_file: PathBuf,
    executable: PathBuf,
    command_line: Vec<OsString>,
    instance: Option<InstanceLock>,
}

impl Startup {
    /// Resolve this run's paths and take the lock before anything writes settings or mods.
    /// Another running instance is a normal exit; an unwritable lock folder still permits startup.
    pub fn open() -> Option<Self> {
        // The arguments as the OS gave them: a folder that is not valid Unicode is still a folder, and a restart passes
        // them on unchanged.
        let command_line: Vec<OsString> = std::env::args_os().skip(1).collect();
        let arguments = Arguments::parse(command_line.iter());
        let executable = std::env::current_exe().unwrap_or_default();
        let folder = executable.parent().unwrap_or(&executable);
        let root = arguments.root.clone().unwrap_or_else(|| paths::find_root(folder));
        // The settings name the log folder. Until they are read, panics go to the root's default; `application::run`
        // then installs the hook again with the settings' folder.
        install_panic_hook(FaultLog::new(&root, None));

        // A restart hands over within the wait. Two writers must never race on the same settings or mods.
        let settings_file = SettingsStore::default_file();
        let instance = match instance::acquire(&settings_file, Duration::from_secs(5)) {
            Ok(None) => {
                eg_ui::system::tell("Eterra's Gift", "Eterra's Gift is already running.");
                return None;
            }
            Ok(Some(lock)) => Some(lock),
            Err(_) => None,
        };
        Some(Self {
            arguments,
            root,
            settings_file,
            executable,
            command_line,
            instance,
        })
    }

    /// Rerun this same executable with the original arguments, after all writers have closed. False, with a fault
    /// report, when the new process could not be started.
    pub fn restart(self, faults: &FaultLog) -> bool {
        // Release first so the new app takes the lock without waiting.
        drop(self.instance);
        match std::process::Command::new(&self.executable).args(&self.command_line).spawn() {
            Ok(_) => true,
            Err(error) => {
                faults.write("Restarting Eterra's Gift", Some(&error.to_string()), None, None);
                false
            }
        }
    }
}

/// Report every panic the trainer worker does not catch itself. One on the main thread then ends the process with a
/// non-zero code and no restart; one on another thread ends that thread, and whoever joins it decides what follows.
/// Installing again replaces the previous hook.
pub fn install_panic_hook(faults: FaultLog) {
    std::panic::set_hook(Box::new(move |panic| {
        if trainer::catches_panic() {
            return;
        }
        let report = format!("{panic}\n{}", std::backtrace::Backtrace::force_capture());
        faults.write("Panic", Some(&report), None, None);
    }));
}
