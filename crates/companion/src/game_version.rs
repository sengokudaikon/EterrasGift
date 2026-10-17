//! The game's own version ("1.5.2"), as it writes it into Player.log near the start of every run ("game version: 1.5.2.
//! internal version: 1.5.2"). The game folder holds it only inside Unity's binary settings, so the last run's log is the
//! plain source.

use regex::Regex;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;
use std::time::SystemTime;

const HEADER_BYTES: u64 = 1024 * 1024;

/// The version from Player.log, or the log of the run before it beside it (Player-prev.log); None when neither says
/// one. A log older than `build` (the game's GameAssembly.dll) is from before an update, so it is not used.
pub fn read(player_log: &Path, build: &Path) -> Option<String> {
    let installed = fs::metadata(build).and_then(|metadata| metadata.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    let previous = player_log.with_file_name("Player-prev.log");
    [player_log, previous.as_path()].into_iter().find_map(|path| {
        // Being written or locked: the other log, or no version.
        let written = fs::metadata(path).and_then(|metadata| metadata.modified()).ok()?;
        if written < installed {
            return None;
        }
        let mut head = Vec::new();
        fs::File::open(path).ok()?.take(HEADER_BYTES).read_to_end(&mut head).ok()?;
        parse(&String::from_utf8_lossy(&head))
    })
}

/// The version in a log's text; None when it has no "game version:" line.
pub fn parse(text: &str) -> Option<String> {
    static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bgame version: (\d+(?:\.\d+)+)").unwrap());
    LINE.captures(text).map(|found| found[1].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_comes_from_the_newest_log_that_is_not_older_than_the_build() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("Player.log");
        let build = temp.path().join("GameAssembly.dll");
        assert_eq!(read(&log, &build), None);
        fs::write(temp.path().join("Player-prev.log"), "Mono path\ngame version: 1.5.1. internal version: 1.5.1\n").unwrap();
        assert_eq!(read(&log, &build).as_deref(), Some("1.5.1"));
        fs::write(&log, "game version: 1.5.2. internal version: 1.5.2\n").unwrap();
        assert_eq!(read(&log, &build).as_deref(), Some("1.5.2"));
        assert_eq!(parse("no version here"), None);
    }
}
