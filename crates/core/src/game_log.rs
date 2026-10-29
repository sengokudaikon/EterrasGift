//! Last Epoch's Player.log, read for what the running game says about itself. The game's own lines carry a UTC
//! timestamp and a level, separated by tabs; engine output and stack traces carry neither. The companion uses the
//! readiness; a module that reaches into the game gates on the same reader.

use jiff::{SignedDuration, Timestamp};
use regex::Regex;
use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GameReadiness {
    #[default]
    Starting,
    NoCharacter,
    Online,
    Offline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggedCharacter {
    pub name: String,
    /// Offline/legacy loads log a level; current online loads log a character ID instead.
    pub level: Option<u32>,
    pub online: bool,
    pub cycle: String,
    pub class: String,
}

/// A Player.log line that changes what the app knows about the running game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameLogEvent {
    /// The shell's scene state: None (in game), SplashScreen, Login or CharacterSelect.
    ShellStateChanged(String),
    CharacterLoaded(LoggedCharacter),
    /// Logged once at startup. False for a normal launch, even when an offline character is played later.
    LaunchModeLogged(bool),
    /// Online client initialization revokes admission even before a character load is logged.
    OnlineClientInitialized,
    GameQuitting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameLogStatus {
    /// No log file.
    Missing,
    /// The log belongs to an earlier run, or has no timestamped line yet.
    Stale,
    Unreadable,
    /// The running game's log, followed and understood.
    Current,
    /// The running game's log, but it shows the game entered play without a character line the parser recognises.
    Unrecognized,
}

/// What the running game's log says. Readiness is Starting until the log shows the login or a character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameLogState {
    pub status: GameLogStatus,
    pub readiness: GameReadiness,
    pub character: Option<LoggedCharacter>,
    pub shell_state: Option<String>,
    pub offline_mode: Option<bool>,
    pub problem: Option<String>,
}

impl GameLogState {
    fn of(status: GameLogStatus) -> GameLogState {
        GameLogState {
            status,
            readiness: GameReadiness::Starting,
            character: None,
            shell_state: None,
            offline_mode: None,
            problem: None,
        }
    }

    pub fn missing() -> GameLogState {
        GameLogState::of(GameLogStatus::Missing)
    }

    pub fn stale() -> GameLogState {
        GameLogState::of(GameLogStatus::Stale)
    }

    pub fn started() -> GameLogState {
        GameLogState::of(GameLogStatus::Current)
    }

    pub fn apply(mut self, event: GameLogEvent) -> GameLogState {
        match event {
            GameLogEvent::LaunchModeLogged(offline) => self.offline_mode = Some(offline),
            GameLogEvent::CharacterLoaded(character) => {
                self.status = GameLogStatus::Current;
                self.readiness = if character.online { GameReadiness::Online } else { GameReadiness::Offline };
                self.character = Some(character);
            }
            GameLogEvent::OnlineClientInitialized => {
                self.status = GameLogStatus::Current;
                self.readiness = GameReadiness::Online;
                if self.character.as_ref().is_some_and(|character| !character.online) {
                    self.character = None;
                }
            }
            GameLogEvent::ShellStateChanged(state) if state == "SplashScreen" => {
                self.shell_state = Some(state);
                self.readiness = GameReadiness::Starting;
                self.character = None;
            }
            GameLogEvent::ShellStateChanged(state) if state == "Login" || state == "CharacterSelect" => {
                self.status = GameLogStatus::Current;
                self.shell_state = Some(state);
                self.readiness = GameReadiness::NoCharacter;
                self.character = None;
            }
            GameLogEvent::ShellStateChanged(state) => {
                // Online initialization is sufficient to refuse access, even if
                // its character line is unknown. Offline admission still needs a load.
                if self.character.is_none() && self.readiness != GameReadiness::Online && matches!(self.shell_state.as_deref(), Some("Login" | "CharacterSelect")) {
                    self.status = GameLogStatus::Unrecognized;
                    self.readiness = GameReadiness::NoCharacter;
                }
                self.shell_state = Some(state);
            }
            GameLogEvent::GameQuitting => {
                self.readiness = GameReadiness::Starting;
                self.character = None;
            }
        }
        self
    }
}

static STAMPED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?<time>\d{4}-\d\d-\d\dT\S+)\t(?<level>[A-Za-z]+)\t(?<message>.*)$").unwrap());
static SHELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ShellSceneManager: State transition (?<from>\w+) -> (?<to>\w+)$").unwrap());
static CHARACTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Loading character: (?<name>.+) (?<level>\d{1,3}) isOnline: (?<online>True|False), cycle: (?<cycle>[^,]*), class: (?<class>.+)$").unwrap());
static ONLINE_CHARACTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Loading character: (?<name>.+) [0-9A-Fa-f]{16} isOnline: True, cycle: (?<cycle>[^,]*), class: (?<class>.+)$").unwrap());
/// The game logs `Offline Mode: False` however it was started (checked live 2026-10-20): the line carries no
/// information, and no gate may use it. Offline play is told by the character's `isOnline` instead.
static LAUNCH_MODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Offline Mode: (?<offline>True|False)$").unwrap());

/// Reads a Player.log line: its time when it is one of the game's own (stamped) lines, and the event it carries, if any.
pub fn parse(line: &str) -> Option<(Timestamp, Option<GameLogEvent>)> {
    let stamped = STAMPED.captures(line)?;
    let time = parse_time(&stamped["time"])?;
    let message = stamped["message"].trim_end();
    let event = if let Some(shell) = SHELL.captures(message) {
        Some(GameLogEvent::ShellStateChanged(shell["to"].to_owned()))
    } else if let Some(character) = CHARACTER.captures(message) {
        Some(GameLogEvent::CharacterLoaded(LoggedCharacter {
            name: character["name"].to_owned(),
            level: Some(character["level"].parse().ok()?),
            online: &character["online"] == "True",
            cycle: character["cycle"].to_owned(),
            class: character["class"].to_owned(),
        }))
    } else if let Some(character) = ONLINE_CHARACTER.captures(message) {
        Some(GameLogEvent::CharacterLoaded(LoggedCharacter {
            name: character["name"].to_owned(),
            level: None,
            online: true,
            cycle: character["cycle"].to_owned(),
            class: character["class"].to_owned(),
        }))
    } else if message == "[Network] Initializing network service group for online client." {
        Some(GameLogEvent::OnlineClientInitialized)
    } else if let Some(mode) = LAUNCH_MODE.captures(message) {
        Some(GameLogEvent::LaunchModeLogged(&mode["offline"] == "True"))
    } else if message == "Application.quitting..." {
        Some(GameLogEvent::GameQuitting)
    } else {
        None
    };
    Some((time, event))
}

/// The game writes ISO-8601 with an offset ("2026-10-06T22:02:45.9948092+00:00"); a time without one is UTC.
fn parse_time(text: &str) -> Option<Timestamp> {
    text.parse::<Timestamp>().ok().or_else(|| {
        text.parse::<jiff::civil::DateTime>()
            .ok()?
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|zoned| zoned.timestamp())
    })
}

/// How far the game's clock and the process start time may disagree before a log counts as an earlier run's.
const CLOCK_SKEW: SignedDuration = SignedDuration::from_secs(10);
const HEADER_BYTES: u64 = 256 * 1024;
/// How much of the log one read takes at a time.
const READ_BYTES: usize = 64 * 1024;

/// Follows Player.log for one game process, reading only what was appended since the last call. A log is the running
/// game's own when its first timestamped line is not older than the process. Not thread-safe; callers serialise reads.
#[derive(Debug)]
pub struct GameLogReader {
    path: PathBuf,
    process_start: Option<Timestamp>,
    first: Option<Timestamp>,
    offset: u64,
    pending: Vec<u8>,
    /// Kept between reads, which come every few seconds while the game runs.
    buffer: Vec<u8>,
    state: GameLogState,
}

impl GameLogReader {
    pub fn new(path: &Path) -> GameLogReader {
        GameLogReader {
            path: path.to_path_buf(),
            process_start: None,
            first: None,
            offset: 0,
            pending: Vec::new(),
            buffer: Vec::new(),
            state: GameLogState::missing(),
        }
    }

    /// What the log says about the game process started at `process_start`.
    pub fn read(&mut self, process_start: Timestamp) -> GameLogState {
        if self.process_start != Some(process_start) {
            self.reset();
            self.process_start = Some(process_start);
        }
        match self.follow(process_start) {
            Ok(state) => state,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.reset();
                self.state.clone()
            }
            // Keep the position: a transient sharing failure resumes where it stopped.
            Err(error) => GameLogState {
                readiness: self.state.readiness,
                problem: Some(error.to_string()),
                ..GameLogState::of(GameLogStatus::Unreadable)
            },
        }
    }

    fn follow(&mut self, process_start: Timestamp) -> io::Result<GameLogState> {
        // Opened with read, write and delete sharing (Rust's default on Windows): the game writes and rotates it.
        let mut file = File::open(&self.path)?;
        if self.first.is_none() || file.metadata()?.len() < self.offset {
            // Not following yet, or the file was replaced: find whose log this is before reading it.
            self.reset();
            match first_stamp(&mut file)? {
                Some(first) if first >= process_start - CLOCK_SKEW => {
                    self.first = Some(first);
                    self.state = GameLogState::started();
                }
                _ => {
                    self.state = GameLogState::stale();
                    return Ok(self.state.clone());
                }
            }
        }
        self.consume(&mut file)?;
        Ok(self.state.clone())
    }

    fn reset(&mut self) {
        self.first = None;
        self.offset = 0;
        self.pending.clear();
        self.state = GameLogState::missing();
    }

    fn consume(&mut self, file: &mut File) -> io::Result<()> {
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.resize(READ_BYTES, 0);
        let result = self.consume_into(file, &mut buffer);
        self.buffer = buffer;
        result
    }

    fn consume_into(&mut self, file: &mut File, buffer: &mut [u8]) -> io::Result<()> {
        loop {
            let read = file.read(buffer)?;
            if read == 0 {
                return Ok(());
            }
            self.offset += read as u64;
            let mut chunk = &buffer[..read];
            while let Some(newline) = chunk.iter().position(|&byte| byte == b'\n') {
                if self.pending.is_empty() {
                    self.line(&chunk[..newline]);
                } else {
                    let mut line = std::mem::take(&mut self.pending);
                    line.extend_from_slice(&chunk[..newline]);
                    self.line(&line);
                    line.clear();
                    self.pending = line;
                }
                chunk = &chunk[newline + 1..];
            }
            self.pending.extend_from_slice(chunk);
        }
    }

    fn line(&mut self, bytes: &[u8]) {
        if let Some(line) = stamped(bytes)
            && let Some((_, Some(event))) = parse(&line)
        {
            self.state = std::mem::replace(&mut self.state, GameLogState::missing()).apply(event);
        }
    }
}

fn first_stamp(file: &mut File) -> io::Result<Option<Timestamp>> {
    let mut head = Vec::new();
    file.take(HEADER_BYTES).read_to_end(&mut head)?;
    let mut lines = head.split(|&byte| byte == b'\n');
    // The last piece has no newline yet; a line counts once it is whole.
    lines.next_back();
    Ok(lines.filter_map(stamped).find_map(|line| parse(&line).map(|(time, _)| time)))
}

/// Decodes only lines that start like a timestamp; most of the log is engine output. Borrowed unless the line is not
/// valid UTF-8.
fn stamped(bytes: &[u8]) -> Option<Cow<'_, str>> {
    bytes.first().filter(|byte| byte.is_ascii_digit())?;
    let end = bytes.iter().rposition(|&byte| byte != b'\r').map_or(0, |last| last + 1);
    Some(String::from_utf8_lossy(&bytes[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn start() -> Timestamp {
        "2026-10-07T10:00:00Z".parse().unwrap()
    }

    fn line(time: Timestamp, message: &str) -> String {
        format!("{}\tLog\t{message}\r\n", time.display_with_offset(jiff::tz::Offset::UTC))
    }

    fn menu(time: Timestamp) -> String {
        line(time, "Offline Mode: False")
            + &line(time, "ShellSceneManager: State transition SplashScreen -> Login")
            + &line(time, "ShellSceneManager: State transition Login -> CharacterSelect")
    }

    fn character(time: Timestamp, online: bool) -> String {
        let online = if online { "True" } else { "False" };
        line(time, &format!("Loading character: Tester 23 isOnline: {online}, cycle: Swordfish, class: Acolyte"))
    }

    fn minutes(count: i64) -> SignedDuration {
        SignedDuration::from_mins(count)
    }

    #[test]
    fn the_parser_reads_the_games_state_lines_and_ignores_engine_output() {
        let (time, loaded) = parse("2026-10-06T22:02:45.9948092+00:00\tLog\tLoading character: Two Words 100 isOnline: True, cycle: Legacy, class: Rogue").unwrap();
        assert_eq!(time, "2026-10-06T22:02:45.9948092Z".parse().unwrap());
        assert_eq!(
            loaded,
            Some(GameLogEvent::CharacterLoaded(LoggedCharacter {
                name: "Two Words".into(),
                level: Some(100),
                online: true,
                cycle: "Legacy".into(),
                class: "Rogue".into(),
            }))
        );
        let event = |text: &str| parse(text).unwrap().1;
        assert_eq!(
            event("2026-10-06T22:02:36.7806112+00:00\tLog\tShellSceneManager: State transition Login -> CharacterSelect"),
            Some(GameLogEvent::ShellStateChanged("CharacterSelect".into()))
        );
        assert_eq!(
            event("2026-10-06T22:02:15.9902499+00:00\tLog\tOffline Mode: True "),
            Some(GameLogEvent::LaunchModeLogged(true))
        );
        assert_eq!(event("2026-10-07T02:24:03.2211553+00:00\tLog\tApplication.quitting..."), Some(GameLogEvent::GameQuitting));
        // Stamped lines without state still identify the log; engine lines and stack traces do not parse at all.
        assert_eq!(
            event("2026-10-06T22:02:21.4932966+00:00\tWarning\tScene PersistentUI load started: load mode: Additive"),
            None
        );
        assert!(parse("LE.Services.OfflineCharacterService:StartPlay(CharacterData, CancellationToken)").is_none());
        assert!(parse("Initialize engine version: 6000.4.8f1 (f8b72d3d7343)").is_none());
    }

    #[test]
    fn the_reader_follows_the_login_and_characters_of_the_running_game() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Player.log");
        let mut reader = GameLogReader::new(&path);
        assert_eq!(reader.read(start()).status, GameLogStatus::Missing);

        fs::write(&path, menu(start())).unwrap();
        let state = reader.read(start());
        assert_eq!(
            (state.status, state.readiness, state.offline_mode),
            (GameLogStatus::Current, GameReadiness::NoCharacter, Some(false))
        );

        // A half-written line waits for its end.
        let append = |text: &str| fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(text.as_bytes()).unwrap();
        let half = character(start() + minutes(1), false);
        let (head, tail) = half.split_at(half.len() - 30);
        append(head);
        assert_eq!(reader.read(start()).readiness, GameReadiness::NoCharacter);
        append(tail);
        append(&line(start() + minutes(1), "ShellSceneManager: State transition CharacterSelect -> None"));
        let state = reader.read(start());
        assert_eq!(state.readiness, GameReadiness::Offline);
        assert_eq!(state.character.unwrap().name, "Tester");

        append(&line(start() + minutes(2), "ShellSceneManager: State transition None -> CharacterSelect"));
        let state = reader.read(start());
        assert_eq!((state.readiness, state.character), (GameReadiness::NoCharacter, None));
        append(&character(start() + minutes(3), true));
        assert_eq!(reader.read(start()).readiness, GameReadiness::Online);
        append(&line(start() + minutes(4), "Application.quitting..."));
        assert_eq!(reader.read(start()).readiness, GameReadiness::Starting);
    }

    #[test]
    fn an_earlier_runs_log_is_stale_until_the_game_replaces_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Player.log");
        fs::write(&path, menu(start() - minutes(180)) + &character(start() - minutes(120), false)).unwrap();
        let mut reader = GameLogReader::new(&path);
        assert_eq!(reader.read(start()).status, GameLogStatus::Stale);

        // The game rotates the log at launch; the new file starts with this run's timestamps.
        fs::rename(&path, temp.path().join("Player-prev.log")).unwrap();
        fs::write(&path, "Initialize engine version: 6000.4.8f1\r\n").unwrap();
        assert_eq!(reader.read(start()).status, GameLogStatus::Stale);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(menu(start() + SignedDuration::from_secs(3)).as_bytes())
            .unwrap();
        assert_eq!(reader.read(start()).readiness, GameReadiness::NoCharacter);

        // A different process start follows its own log from the beginning.
        assert_eq!(reader.read(start() + minutes(60)).status, GameLogStatus::Stale);
    }

    #[test]
    fn entering_play_without_a_recognised_character_line_is_unrecognized() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Player.log");
        fs::write(
            &path,
            menu(start())
                + &line(start() + minutes(1), "Loading hero: Tester 23 (offline)")
                + &line(start() + minutes(1), "ShellSceneManager: State transition CharacterSelect -> None"),
        )
        .unwrap();
        let state = GameLogReader::new(&path).read(start());
        assert_eq!(state.status, GameLogStatus::Unrecognized);
        assert_ne!(state.readiness, GameReadiness::Offline);
    }

    #[test]
    fn online_character_ids_are_not_levels_and_stay_online_when_entering_play() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Player.log");
        fs::write(
            &path,
            menu(start())
                + &line(start(), "Loading character: Online Mage B6C126989DAFF026 isOnline: True, cycle: Swordfish, class: Mage")
                + &line(start(), "[Network] Initializing network service group for online client.")
                + &line(start(), "ShellSceneManager: State transition CharacterSelect -> None"),
        )
        .unwrap();
        let state = GameLogReader::new(&path).read(start());
        assert_eq!((state.status, state.readiness), (GameLogStatus::Current, GameReadiness::Online));
        let character = state.character.unwrap();
        assert_eq!(character.name, "Online Mage");
        assert_eq!(character.level, None);
        assert!(character.online);
        assert_eq!(state.offline_mode, Some(false));
    }

    #[test]
    fn online_client_selection_revokes_an_old_offline_character_without_a_recognized_load() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Player.log");
        fs::write(
            &path,
            menu(start())
                + &character(start(), false)
                + &line(start(), "ShellSceneManager: State transition CharacterSelect -> None")
                + &line(start(), "[Network] Initializing network service group for online client.")
                + &line(start(), "ShellSceneManager: State transition None -> None"),
        )
        .unwrap();
        let state = GameLogReader::new(&path).read(start());
        assert_eq!((state.status, state.readiness, state.character), (GameLogStatus::Current, GameReadiness::Online, None));

        // Creating an online character need not produce the ordinary load line.
        fs::write(
            &path,
            menu(start())
                + &line(start(), "[Network] Initializing network service group for online client.")
                + &line(start(), "Character Created: Online Mage B6C126989DAFF026 isOnline: True, cycle: Swordfish, class: 1")
                + &line(start(), "ShellSceneManager: State transition CharacterSelect -> None"),
        )
        .unwrap();
        let state = GameLogReader::new(&path).read(start());
        assert_eq!((state.status, state.readiness), (GameLogStatus::Current, GameReadiness::Online));
    }

    #[test]
    fn leaving_online_requires_a_recognized_offline_load_before_admission() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Player.log");
        fs::write(&path, menu(start()) + &line(start(), "[Network] Initializing network service group for online client.")).unwrap();
        let mut reader = GameLogReader::new(&path);
        assert_eq!(reader.read(start()).readiness, GameReadiness::Online);
        let append = |message| {
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(line(start(), message).as_bytes())
                .unwrap()
        };
        append("ShellSceneManager: State transition None -> CharacterSelect");
        append("[Network] Initializing network service group for offline client.");
        assert_eq!(reader.read(start()).readiness, GameReadiness::NoCharacter);
        // A hexadecimal ID must never acquire the meaning of an offline level.
        append("Loading character: Bad Offline B6C126989DAFF026 isOnline: False, cycle: Swordfish, class: Mage");
        append("ShellSceneManager: State transition CharacterSelect -> None");
        assert_eq!(reader.read(start()).status, GameLogStatus::Unrecognized);
        append("Loading character: Offline Mage 27 isOnline: False, cycle: Swordfish, class: Mage");
        assert_eq!(reader.read(start()).readiness, GameReadiness::Offline);
    }

    #[test]
    fn lines_past_the_header_are_read_in_the_same_pass() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Player.log");
        let mut game = fs::File::create(&path).unwrap();
        let mut noise = menu(start());
        let mut index = 0;
        while noise.len() <= HEADER_BYTES as usize {
            noise.push_str("LE.AssetBundles.<SceneLoadAsyncUpdate>d__103:MoveNext()\r\n");
            noise.push_str(&line(
                start() + SignedDuration::from_secs(5),
                &format!("Scene M_M{index} load started: load mode: Additive"),
            ));
            index += 1;
        }
        noise.push_str(&character(start() + minutes(1), false));
        game.write_all(noise.as_bytes()).unwrap();
        game.flush().unwrap();
        let state = GameLogReader::new(&path).read(start());
        assert_eq!((state.status, state.readiness), (GameLogStatus::Current, GameReadiness::Offline));
    }
}
