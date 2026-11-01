//! In-app diagnostics: the current trainer state, recent fault reports and a bounded tail of Player.log.
//! Paths are composition inputs. Reads run on the existing background service only while the page is visible;
//! source changes invalidate older reads, and unchanged refreshes leave text selection alone.

use crate::{
    LogsState, MainWindow, background,
    shell::{Page, ShellHandle},
};
use eg_i18n::phrases;
use eg_trainer::backend::BackendHandle;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::Cell,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

phrases! {
    App: "Eterra's Gift"
    Game: "Last Epoch"
    NoSources: "No log files are configured for this run."
    NoReports: "No fault reports have been written."
    Empty: "This log is empty."
    Recent: "Showing the latest part of the log. Refreshes every two seconds while open."
    Refreshes: "Refreshes every two seconds while open."
    RecentReports: "Showing up to eight recent fault reports. Refreshes every two seconds while open."
    ReadFailed: "Could not read {path}: {error}"
    Connection: "Trainer connection\nState: {phase}\nProcess: {process}\nGame log: {gate}\nError: {error}\n"
    NoTrainer: "The trainer is not running."
}

const TAIL_BYTES: usize = 256 * 1024;
const REPORT_BYTES: usize = 32 * 1024;
const REPORT_COUNT: usize = 8;

#[derive(Clone, Debug)]
pub struct LogFiles {
    pub game: PathBuf,
    /// The primary fault-report folder followed by its write fallbacks.
    pub reports: Vec<PathBuf>,
}

struct Contents {
    location: String,
    text: String,
    notice: String,
}

/// Reads at most `limit` bytes, discarding a partial first line when starting in the middle of a file.
fn tail(path: &Path, limit: usize) -> io::Result<(String, bool)> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(limit as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(limit.min(length as usize));
    file.take(limit as u64).read_to_end(&mut bytes)?;
    let bytes = if start > 0 {
        bytes.iter().position(|&byte| byte == b'\n').map_or(&bytes[..], |newline| &bytes[newline + 1..])
    } else {
        &bytes
    };
    Ok((String::from_utf8_lossy(bytes).into_owned(), start > 0))
}

fn read_error(path: &Path, error: &io::Error) -> String {
    ReadFailed.with(&[("path", &path.display()), ("error", error)])
}

impl LogFiles {
    fn game(&self) -> Contents {
        let (text, notice) = match tail(&self.game, TAIL_BYTES) {
            Ok((text, truncated)) => {
                let notice = if text.is_empty() {
                    Empty
                } else if truncated {
                    Recent
                } else {
                    Refreshes
                };
                (text, notice.get().to_owned())
            }
            Err(error) => (String::new(), read_error(&self.game, &error)),
        };
        Contents {
            location: self.game.display().to_string(),
            text,
            notice,
        }
    }

    fn app(&self, connection: String) -> Contents {
        let mut reports = Vec::new();
        let mut problems = Vec::new();
        let mut directories = Vec::new();
        for directory in &self.reports {
            if directories.contains(directory) {
                continue;
            }
            directories.push(directory.clone());
            let entries = match fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    problems.push(read_error(directory, &error));
                    continue;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        problems.push(read_error(directory, &error));
                        break;
                    }
                };
                let path = entry.path();
                if path.extension().is_some_and(|extension| extension == "json")
                    && entry.file_name().to_string_lossy().starts_with("fault-")
                    && entry.file_type().is_ok_and(|kind| kind.is_file())
                {
                    reports.push((entry.file_name(), path));
                    reports.sort_by(|a, b| b.0.cmp(&a.0));
                    reports.truncate(REPORT_COUNT);
                }
            }
        }
        let mut sections = vec![connection];
        if reports.is_empty() {
            sections.push(NoReports.get().to_owned());
        }
        for (_, path) in reports {
            let text = match tail(&path, REPORT_BYTES) {
                Ok((text, _)) => text,
                Err(error) => read_error(&path, &error),
            };
            sections.push(format!("{}\n{text}", path.display()));
        }
        sections.extend(problems);
        Contents {
            location: directories.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join("\n"),
            text: sections.join("\n\n"),
            notice: RecentReports.get().to_owned(),
        }
    }
}

struct Binding {
    window: slint::Weak<MainWindow>,
    files: Option<LogFiles>,
    backend: Option<BackendHandle>,
    shell: ShellHandle,
    generation: Cell<u64>,
    timer: slint::Timer,
}

pub(crate) fn bind(window: &MainWindow, files: Option<LogFiles>, backend: Option<BackendHandle>, shell: &ShellHandle) {
    let binding = Rc::new(Binding {
        window: window.as_weak(),
        files,
        backend,
        shell: shell.clone(),
        generation: Cell::new(0),
        timer: slint::Timer::default(),
    });
    window
        .global::<LogsState>()
        .set_sources(ModelRc::new(VecModel::from(vec![App.get().into(), Game.get().into()])));
    window.global::<LogsState>().on_refresh({
        let binding = binding.clone();
        move || binding.refresh(false)
    });
    window.global::<LogsState>().on_source_chosen({
        let binding = binding.clone();
        move |source| {
            if (0..=1).contains(&source)
                && let Some(window) = binding.window.upgrade()
            {
                window.global::<LogsState>().set_source(source);
                binding.refresh(true);
            }
        }
    });
    shell.on_page_entered({
        let binding = Rc::downgrade(&binding);
        move |page| {
            if page == Page::Logs
                && let Some(binding) = binding.upgrade()
            {
                binding.refresh(false);
            }
        }
    });
    binding.timer.start(slint::TimerMode::Repeated, Duration::from_secs(2), {
        let binding = Rc::downgrade(&binding);
        move || {
            if let Some(binding) = binding.upgrade()
                && binding.shell.current() == Page::Logs
                && binding.window.upgrade().is_some_and(|window| !window.global::<LogsState>().get_loading())
            {
                binding.refresh(false);
            }
        }
    });
    if shell.current() == Page::Logs {
        binding.refresh(false);
    }
}

impl Binding {
    fn connection(&self) -> String {
        self.backend.as_ref().map_or_else(
            || NoTrainer.get().to_owned(),
            |backend| {
                backend.peek(|snapshot| {
                    Connection.with(&[
                        ("phase", &format!("{:?}", snapshot.phase)),
                        ("process", &format!("{:?}", snapshot.detected)),
                        ("gate", &format!("{:?}", snapshot.gate)),
                        ("error", &format!("{:?}", snapshot.error)),
                    ])
                })
            },
        )
    }

    fn refresh(self: &Rc<Self>, clear: bool) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<LogsState>();
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        if clear {
            state.set_text("".into());
            state.set_location("".into());
        }
        let Some(files) = self.files.clone() else {
            state.set_notice(NoSources.get().into());
            state.set_text(if state.get_source() == 0 { self.connection().into() } else { "".into() });
            state.set_loading(false);
            return;
        };
        let source = state.get_source();
        let connection = self.connection();
        state.set_loading(true);
        let binding = Rc::downgrade(self);
        background::run(
            move || if source == 1 { files.game() } else { files.app(connection) },
            move |contents| {
                let Some(binding) = binding.upgrade().filter(|binding| binding.generation.get() == generation) else {
                    return;
                };
                if let Some(window) = binding.window.upgrade() {
                    let state = window.global::<LogsState>();
                    if state.get_text().as_str() != contents.text {
                        state.set_text(contents.text.into());
                    }
                    state.set_location(contents.location.into());
                    state.set_notice(contents.notice.into());
                    state.set_loading(false);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn bounded_tail_follows_appends_truncation_and_missing_files() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("Player.log");
        fs::write(&path, "old line\nnew line\n").unwrap();
        assert_eq!(tail(&path, 12).unwrap(), ("new line\n".into(), true));
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all("最後\n".as_bytes()).unwrap();
        assert_eq!(tail(&path, 12).unwrap(), ("最後\n".into(), true));
        fs::write(&path, "fresh\n").unwrap();
        assert_eq!(tail(&path, 12).unwrap(), ("fresh\n".into(), false));
        fs::remove_file(&path).unwrap();
        assert_eq!(tail(&path, 12).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn reports_are_recent_bounded_and_include_fallbacks_without_duplicates() {
        let folder = tempfile::tempdir().unwrap();
        let primary = folder.path().join("primary");
        let fallback = folder.path().join("fallback");
        fs::create_dir(&primary).unwrap();
        fs::create_dir(&fallback).unwrap();
        for index in 0..10 {
            fs::write(primary.join(format!("fault-{index:02}.json")), format!("report {index:02}")).unwrap();
        }
        fs::write(fallback.join("fault-99.json"), "fallback report").unwrap();
        fs::write(primary.join("other.json"), "not a fault").unwrap();
        let files = LogFiles {
            game: folder.path().join("missing.log"),
            reports: vec![primary.clone(), primary, fallback],
        };
        let contents = files.app("current connection error".into());
        assert!(contents.text.starts_with("current connection error"));
        assert_eq!(contents.text.matches(".json").count(), REPORT_COUNT);
        assert!(contents.text.contains("fallback report"));
        assert!(!contents.text.contains("report 02"));
        assert!(!contents.text.contains("not a fault"));
        assert!(files.game().text.is_empty());
        assert!(files.game().notice.contains("Could not read"));
    }
}
