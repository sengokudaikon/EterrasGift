//! The log viewer: recent fault reports and a bounded tail of Player.log, as a feature.
//!
//! Showing the page, a tick while it shows and the user's choices are intents; reading the files is an effect. A newer read
//! makes an older one's result meaningless, so each read carries a generation. Contents that come back unchanged leave
//! the view alone, which keeps a text selection in place.

use phrases::phrases;
use spiderland::{Effects, Error, Feature, Tracked};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};

phrases! {
    NoSources: "No log files are configured for this run."
    NoReports: "No fault reports have been written."
    Empty: "This log is empty."
    Recent: "Showing the latest part of the log. Refreshes every two seconds while open."
    Refreshes: "Refreshes every two seconds while open."
    RecentReports: "Showing up to eight recent fault reports. Refreshes every two seconds while open."
    ReadFailed: "Could not read {path}: {error}"
    ReadPanicked: "Could not read the logs. Details are in the fault log."
}

/// How often the page reads the files again while it shows.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(2);

const TAIL_BYTES: usize = 256 * 1024;
const REPORT_BYTES: usize = 32 * 1024;
const REPORT_COUNT: usize = 8;

/// Which log the page shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// This app's state and fault reports.
    #[default]
    App,
    /// The game's Player.log.
    Game,
}

#[derive(Clone, Debug)]
pub struct LogFiles {
    pub game: PathBuf,
    /// The primary fault-report folder followed by its write fallbacks.
    pub reports: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    pub location: String,
    pub text: String,
    pub notice: String,
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
            location: eg_core::paths::mask_profile(&self.game.display().to_string()),
            text: eg_core::paths::mask_profile(&text),
            notice,
        }
    }

    fn app(&self, status: String) -> Contents {
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
        let mut sections = vec![status];
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
            location: eg_core::paths::mask_profile(&directories.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join("\n")),
            text: eg_core::paths::mask_profile(&sections.join("\n\n")),
            notice: RecentReports.get().to_owned(),
        }
    }
}

/// Reads `source`, so it runs off the UI thread. `status` opens the app's log: what the composition says about the app's
/// live state, in words of its own. Without `files` only `status` can be shown.
pub fn read(files: Option<&LogFiles>, source: Source, status: String) -> Contents {
    match (files, source) {
        (Some(files), Source::Game) => files.game(),
        (Some(files), Source::App) => files.app(status),
        (None, Source::App) => Contents {
            text: status,
            notice: NoSources.get().to_owned(),
            ..Contents::default()
        },
        (None, Source::Game) => Contents {
            notice: NoSources.get().to_owned(),
            ..Contents::default()
        },
    }
}

/// The log viewer as a feature: see the module's documentation.
pub struct Logs;

/// Named for the fault trail (`eg_core::fault::TRAIL`).
#[derive(strum::IntoStaticStr)]
pub enum Intent {
    /// The page came into view.
    Shown,
    /// Another page came into view.
    Hidden,
    /// Time to read again, while the page shows.
    Watch,
    Refresh,
    Choose(Source),
    Read {
        generation: u64,
        result: Result<Contents, Error>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Read { generation: u64, source: Source },
}

#[derive(Debug, Default)]
pub struct State {
    source: Source,
    visible: bool,
    generation: u64,
    loading: bool,
    contents: Tracked<Contents>,
}

impl State {
    /// A page that shows from the first, or not: `start` reads it at once if it does.
    pub fn new(shown: bool) -> State {
        State {
            visible: shown,
            ..State::default()
        }
    }

    pub fn source(&self) -> Source {
        self.source
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// What the view shows. Its version moves only when they changed.
    pub fn contents(&self) -> &Tracked<Contents> {
        &self.contents
    }
}

/// What runs while the state wants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subscription {
    /// A tick every [`WATCH_INTERVAL`], sending [`Intent::Watch`]. The page reads the files only while it shows.
    Tick,
}

impl Feature for Logs {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;
    type Output = spiderland::Never;
    type Subscription = Subscription;

    fn subscriptions(state: &State) -> impl IntoIterator<Item = Subscription> {
        state.visible.then_some(Subscription::Tick)
    }

    fn start(state: &mut State, effects: &mut Effects<Effect>) {
        if state.visible {
            read_again(state, effects);
        }
    }

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect>) {
        match intent {
            Intent::Shown => {
                state.visible = true;
                read_again(state, effects);
            }
            Intent::Hidden => state.visible = false,
            // A slow disk gets to finish: only asking for it by hand replaces a read still out.
            Intent::Watch => {
                if state.visible && !state.loading {
                    read_again(state, effects);
                }
            }
            Intent::Refresh => read_again(state, effects),
            Intent::Choose(source) => {
                state.source = source;
                // The other log's text must not show under this one's name while it loads.
                let notice = state.contents.notice.clone();
                state.contents.set(Contents { notice, ..Contents::default() });
                read_again(state, effects);
            }
            Intent::Read { generation, result } => {
                if generation != state.generation {
                    return;
                }
                state.loading = false;
                match result {
                    Ok(contents) => state.contents.set(contents),
                    // What was last read stays up, with word that it may be out of date.
                    Err(_) => {
                        let mut contents = (*state.contents).clone();
                        contents.notice = ReadPanicked.into();
                        state.contents.set(contents);
                    }
                }
            }
        }
    }
}

fn read_again(state: &mut State, effects: &mut Effects<Effect>) {
    state.generation = state.generation.wrapping_add(1);
    state.loading = true;
    effects.push(Effect::Read {
        generation: state.generation,
        source: state.source,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiderland::step;
    use std::io::Write;

    fn send(state: &mut State, intent: Intent) -> Vec<Effect> {
        step::<Logs>(state, intent).effects
    }

    fn contents(text: &str) -> Contents {
        Contents {
            location: "here".into(),
            text: text.into(),
            notice: "said".into(),
        }
    }

    fn generation(effects: &[Effect]) -> u64 {
        let [Effect::Read { generation, .. }] = effects else {
            panic!("one read expected, not {effects:?}");
        };
        *generation
    }

    #[test]
    fn a_page_that_shows_from_the_first_reads_at_once() {
        assert!(spiderland::start::<Logs>(&mut State::new(false)).is_empty());
        let mut state = State::new(true);
        assert_eq!(generation(&spiderland::start::<Logs>(&mut state)), 1);
        assert!(state.is_loading());
    }

    #[test]
    fn showing_reads_and_the_result_ends_the_wait() {
        let mut state = State::default();
        let effects = send(&mut state, Intent::Shown);
        assert!(state.is_loading());
        assert_eq!(spiderland::subscriptions::<Logs>(&state), [Subscription::Tick]);
        send(
            &mut state,
            Intent::Read {
                generation: generation(&effects),
                result: Ok(contents("text")),
            },
        );
        assert!(!state.is_loading());
        assert_eq!(state.contents().text, "text");
    }

    #[test]
    fn a_tick_waits_for_a_read_still_out_and_a_hidden_page_reads_nothing() {
        let mut state = State::default();
        let effects = send(&mut state, Intent::Shown);
        assert!(send(&mut state, Intent::Watch).is_empty(), "a read is still out");
        send(
            &mut state,
            Intent::Read {
                generation: generation(&effects),
                result: Ok(contents("text")),
            },
        );
        assert_eq!(send(&mut state, Intent::Watch).len(), 1);
        send(&mut state, Intent::Hidden);
        assert!(spiderland::subscriptions::<Logs>(&state).is_empty());
        state.loading = false;
        assert!(send(&mut state, Intent::Watch).is_empty(), "a tick already on its way is ignored");
    }

    #[test]
    fn a_result_from_before_a_newer_read_is_dropped() {
        let mut state = State::default();
        let first = generation(&send(&mut state, Intent::Shown));
        let second = generation(&send(&mut state, Intent::Refresh));
        send(
            &mut state,
            Intent::Read {
                generation: first,
                result: Ok(contents("old")),
            },
        );
        assert!(state.is_loading(), "the newer read is still out");
        assert_eq!(state.contents().text, "");
        send(
            &mut state,
            Intent::Read {
                generation: second,
                result: Ok(contents("new")),
            },
        );
        assert_eq!(state.contents().text, "new");
    }

    #[test]
    fn choosing_a_source_clears_the_old_text_and_reads_the_new_one() {
        let mut state = State::default();
        let first = generation(&send(&mut state, Intent::Shown));
        send(
            &mut state,
            Intent::Read {
                generation: first,
                result: Ok(contents("the app's")),
            },
        );
        let effects = send(&mut state, Intent::Choose(Source::Game));
        assert!(matches!(effects[..], [Effect::Read { source: Source::Game, .. }]));
        assert_eq!(state.contents().text, "");
        assert_eq!(state.contents().location, "");
    }

    #[test]
    fn an_unchanged_read_leaves_the_view_alone() {
        let mut state = State::default();
        let first = generation(&send(&mut state, Intent::Shown));
        send(
            &mut state,
            Intent::Read {
                generation: first,
                result: Ok(contents("same")),
            },
        );
        let shown = state.contents().version();
        let again = generation(&send(&mut state, Intent::Watch));
        send(
            &mut state,
            Intent::Read {
                generation: again,
                result: Ok(contents("same")),
            },
        );
        assert_eq!(state.contents().version(), shown, "a selection in the text survives");
    }

    #[test]
    fn a_read_that_panics_keeps_the_text_and_says_so() {
        let mut state = State::default();
        let first = generation(&send(&mut state, Intent::Shown));
        send(
            &mut state,
            Intent::Read {
                generation: first,
                result: Ok(contents("kept")),
            },
        );
        let again = generation(&send(&mut state, Intent::Refresh));
        send(
            &mut state,
            Intent::Read {
                generation: again,
                result: Err(Error::Panicked),
            },
        );
        assert!(!state.is_loading());
        assert_eq!(state.contents().text, "kept");
        assert_eq!(state.contents().notice, ReadPanicked.get());
    }

    #[test]
    fn without_files_only_the_status_shows() {
        let app = read(None, Source::App, "status".into());
        assert_eq!(app.text, "status");
        assert_eq!(app.notice, NoSources.get());
        assert_eq!(read(None, Source::Game, "status".into()).text, "");
    }

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
        let contents = files.app("current status".into());
        assert!(contents.text.starts_with("current status"));
        assert_eq!(contents.text.matches(".json").count(), REPORT_COUNT);
        assert!(contents.text.contains("fallback report"));
        assert!(!contents.text.contains("report 02"));
        assert!(!contents.text.contains("not a fault"));
        assert!(files.game().text.is_empty());
        assert!(files.game().notice.contains("Could not read"));
    }
}
