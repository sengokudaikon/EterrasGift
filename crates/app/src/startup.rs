//! Process setup and restart. The instance lock stays held until shutdown has closed every writer.

use crate::trainer;
use eg_core::fault::FaultLog;
use eg_core::instance::{self, InstanceLock};
use eg_core::paths;
use eg_core::settings::SettingsStore;
use std::path::PathBuf;
use std::time::Duration;

/// The app's command line (`spec/shell-and-settings.md` §1).
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
    fn parse(mut arguments: impl Iterator<Item = String>) -> Self {
        let mut parsed = Self::default();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--root" => parsed.root = arguments.next().map(PathBuf::from),
                "--settings" => parsed.settings = true,
                "--no-connect" => parsed.no_connect = true,
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
    command_line: Vec<String>,
    instance: Option<InstanceLock>,
}

impl Startup {
    /// Resolve this run's paths and take the lock before anything writes settings or mods.
    /// Another running instance is a normal exit; an unwritable lock folder still permits startup.
    pub fn open() -> Option<Self> {
        let command_line: Vec<String> = std::env::args().skip(1).collect();
        let arguments = Arguments::parse(command_line.iter().cloned());
        let executable = std::env::current_exe().unwrap_or_default();
        let folder = executable.parent().unwrap_or(&executable);
        let root = arguments.root.clone().unwrap_or_else(|| paths::find_root(folder));
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

    /// Rerun this same build with the original arguments, after all writers have closed.
    pub fn restart(self, faults: &FaultLog) {
        // Release first so the new app takes the lock without waiting. A checkout's build is cargo's to make.
        drop(self.instance);
        if let Err(error) = std::process::Command::new(&self.executable).args(&self.command_line).spawn() {
            faults.write("Restarting Eterra's Gift", Some(&error.to_string()), None, None);
            std::process::exit(1);
        }
    }
}

fn install_panic_hook(faults: FaultLog) {
    // A panic anywhere is reported before the process ends with a non-zero code, without restarting it.
    std::panic::set_hook(Box::new(move |panic| {
        if trainer::catches_panic() {
            return;
        }
        let report = format!("{panic}\n{}", std::backtrace::Backtrace::force_capture());
        faults.write("Panic", Some(&report), None, None);
    }));
}
