//! Best-effort fault evidence. Writing a report never fails and never stands in the way of recovery or exit.

use crate::paths;
use jiff::Timestamp;
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError, TryLockError};

/// What a module knows about a fault, for its report: an error code and its details, what became of a change it sent,
/// and the state it expected, the change and what it read back. Each object is written as it is.
#[derive(Clone, Debug, Default)]
pub struct FaultDetails {
    pub code: Option<String>,
    pub details: Option<String>,
    pub write_outcome: Option<String>,
    pub expected: Option<Value>,
    pub edit: Option<Value>,
    pub readback: Option<Value>,
}

/// Writes fault reports for the app at `root`: into the run's log folder, else the fallbacks.
#[derive(Clone, Debug)]
pub struct FaultLog {
    root: PathBuf,
    log_directory: Option<PathBuf>,
    fallbacks: Vec<PathBuf>,
}

impl FaultLog {
    /// `log_directory` is the run's chosen folder; without one, the root's default. Then `%LOCALAPPDATA%`'s, then the
    /// temp folder's.
    pub fn new(root: &Path, log_directory: Option<&Path>) -> FaultLog {
        FaultLog::with_fallbacks(
            root,
            log_directory,
            vec![paths::local_directory().join("logs"), std::env::temp_dir().join(paths::LOCAL_FOLDER).join("logs")],
        )
    }

    /// Tests pass their own fallbacks so they never write into the user's profile.
    pub fn with_fallbacks(root: &Path, log_directory: Option<&Path>, fallbacks: Vec<PathBuf>) -> FaultLog {
        FaultLog {
            root: root.to_path_buf(),
            log_directory: log_directory.map(Path::to_path_buf),
            fallbacks,
        }
    }

    /// The chosen report folder followed by the recovery locations, in write order.
    pub fn directories(&self) -> impl Iterator<Item = PathBuf> + '_ {
        let first = self.log_directory.clone().unwrap_or_else(|| paths::app_paths(&self.root, None, None).log_directory());
        std::iter::once(first).chain(self.fallbacks.iter().cloned())
    }

    /// Writes a report; returns where, or None when no folder took it.
    /// `failure` is the full text of what went wrong (an error and its causes, or a panic with its backtrace).
    pub fn write(&self, operation: &str, failure: Option<&str>, details: Option<&FaultDetails>, recovery: Option<&str>) -> Option<PathBuf> {
        let now = Timestamp::now();
        let mut report = Map::new();
        report.insert("time".into(), now.display_with_offset(jiff::tz::Offset::UTC).to_string().into());
        report.insert("processId".into(), std::process::id().into());
        report.insert("operation".into(), operation.into());
        let details = details.cloned().unwrap_or_default();
        let mut optional = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                report.insert(key.into(), value);
            }
        };
        optional("code", details.code.map(Value::from));
        optional("details", details.details.map(Value::from));
        optional("exception", failure.map(Value::from));
        optional("writeOutcome", details.write_outcome.map(Value::from));
        optional("recovery", recovery.map(Value::from));
        optional("expected", details.expected);
        optional("edit", details.edit);
        optional("readback", details.readback);
        let trail = TRAIL.lines();
        optional("trail", (!trail.is_empty()).then(|| trail.into()));
        report.insert(
            "application".into(),
            json!({
                "executable": std::env::current_exe().ok().map(|path| path.to_string_lossy().into_owned()),
                "version": env!("CARGO_PKG_VERSION"),
            }),
        );
        let mut report = Value::Object(report);
        mask_strings(&mut report);
        let text = serde_json::to_string_pretty(&report).ok()?;
        let name = format!("fault-{}-{}.json", now.strftime("%Y%m%d-%H%M%S"), uuid::Uuid::new_v4().simple());
        self.directories().find_map(|directory| {
            std::fs::create_dir_all(&directory).ok()?;
            let path = directory.join(&name);
            let mut file = OpenOptions::new().write(true).create_new(true).open(&path).ok()?;
            file.write_all(text.as_bytes()).ok()?;
            file.sync_all().ok()?;
            Some(path)
        })
    }
}

/// What the window did last, in every report: the intents its pages ran, in order, each by its name alone, since one can carry
/// a whole snapshot. The window's stores add to it as they run, so a fault in a page's work names the intent it was on, or
/// the last one before.
pub static TRAIL: Trail = Trail::new();

/// The last [`Trail::LENGTH`] intents, one that runs again straight after itself counted rather than kept twice.
pub struct Trail(Mutex<VecDeque<Step>>);

struct Step {
    at: Timestamp,
    page: &'static str,
    intent: &'static str,
    times: u32,
}

impl Trail {
    const LENGTH: usize = 48;

    const fn new() -> Trail {
        Trail(Mutex::new(VecDeque::new()))
    }

    /// Adds `page`'s `intent`, as it is about to run.
    pub fn record(&self, page: &'static str, intent: &'static str) {
        let mut steps = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let at = Timestamp::now();
        if let Some(last) = steps.back_mut()
            && (last.page, last.intent) == (page, intent)
        {
            last.at = at;
            last.times = last.times.saturating_add(1);
            return;
        }
        if steps.len() == Self::LENGTH {
            steps.pop_front();
        }
        steps.push_back(Step { at, page, intent, times: 1 });
    }

    /// The intents, oldest first, as a report writes them. None while another thread is adding one, so a report never waits.
    pub fn lines(&self) -> Vec<String> {
        let steps = match self.0.try_lock() {
            Ok(steps) => steps,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Vec::new(),
        };
        steps
            .iter()
            .map(|step| {
                let times = if step.times > 1 { format!(" ({} times)", step.times) } else { String::new() };
                format!("{} {} {}{times}", step.at.strftime("%H:%M:%S%.3f"), step.page, step.intent)
            })
            .collect()
    }
}

impl std::fmt::Debug for Trail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.lines()).finish()
    }
}

/// Every string in a report, the executable's path, errors and backtraces among them, with the profile folder masked.
fn mask_strings(value: &mut Value) {
    match value {
        Value::String(text) => *text = paths::mask_profile(text),
        Value::Array(items) => items.iter_mut().for_each(mask_strings),
        Value::Object(fields) => fields.values_mut().for_each(mask_strings),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_goes_to_the_first_folder_that_takes_it() {
        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("blocked");
        std::fs::write(&blocked, b"a file where the folder would be").unwrap();
        let fallback = temp.path().join("fallback");
        let log = FaultLog::with_fallbacks(temp.path(), Some(&blocked), vec![fallback.clone()]);
        let path = log.write("Startup", Some("it broke"), None, Some("Eterra's Gift runs without it.")).unwrap();
        assert_eq!(path.parent(), Some(fallback.as_path()));
        let report: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(report["operation"], "Startup");
        assert_eq!(report["exception"], "it broke");
        assert_eq!(report["recovery"], "Eterra's Gift runs without it.");
        assert!(report.get("code").is_none());
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("fault-"));
        let profile = std::env::var("USERPROFILE").unwrap();
        let path = log.write("Startup", Some(&format!(r"{profile}\AppData\x.dll is missing")), None, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&profile.replace('\\', "\\\\")) && text.contains(r"%USERPROFILE%\\AppData\\x.dll"), "{text}");
    }

    #[test]
    fn the_trail_keeps_the_last_intents_and_counts_one_that_runs_again() {
        let trail = Trail::new();
        trail.record("logs", "Watch");
        trail.record("logs", "Watch");
        trail.record("forge", "Observed");
        let lines = trail.lines();
        assert!(lines[0].ends_with(" logs Watch (2 times)") && lines[1].ends_with(" forge Observed"), "{lines:?}");
        for _ in 0..Trail::LENGTH {
            trail.record("forge", "Apply");
            trail.record("forge", "Observed");
        }
        let lines = trail.lines();
        assert_eq!(lines.len(), Trail::LENGTH);
        assert!(lines.last().unwrap().ends_with(" forge Observed"));
    }

    #[test]
    fn a_report_carries_the_trail() {
        let temp = tempfile::tempdir().unwrap();
        let log = FaultLog::with_fallbacks(temp.path(), Some(temp.path()), vec![]);
        TRAIL.record("a test", "WroteAReport");
        let report: Value = serde_json::from_slice(&std::fs::read(log.write("Test", None, None, None).unwrap()).unwrap()).unwrap();
        let trail = report["trail"].as_array().unwrap();
        assert!(trail.iter().any(|line| line.as_str().unwrap().ends_with(" a test WroteAReport")), "{trail:?}");
    }
}
