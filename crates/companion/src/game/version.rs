//! A display version recorded in Player.log ("game version: 1.5.2. internal version: 1.5.2"). Log freshness is checked
//! against the installed build's write time; this is a best-effort label, not proof of the installed build's identity.

use file_ops::stamp;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

const HEADER_BYTES: u64 = 1024 * 1024;

/// Prefer Player.log, then Player-prev.log beside it. Only logs at least as new as `build` (GameAssembly.dll) qualify.
/// None when the build's write time cannot be read, or neither qualifying log has a readable version.
pub fn read(player_log: &Path, build: &Path) -> Option<String> {
    let installed = stamp::written(build).ok()?;
    let previous = player_log.with_file_name("Player-prev.log");
    [player_log, previous.as_path()].into_iter().find_map(|path| {
        // Being written or locked: the other log, or no version.
        if stamp::written(path).ok()? < installed {
            return None;
        }
        let head = file_ops::read::head(path, HEADER_BYTES).ok()?;
        parse(&String::from_utf8_lossy(&head))
    })
}

/// The dotted numeric version following "game version:" anywhere in the log text.
fn parse(text: &str) -> Option<String> {
    static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bgame version: (\d+(?:\.\d+)+)").unwrap());
    LINE.captures(text).map(|found| found[1].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn write_at(path: &Path, text: &str, seconds: u64) {
        fs::write(path, text).unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
    }

    #[test]
    fn logs_cannot_supply_a_version_without_the_build_write_time() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("Player.log");
        let build = temp.path().join("GameAssembly.dll");
        write_at(&log, "game version: 1.5.2", 2000);
        write_at(&temp.path().join("Player-prev.log"), "game version: 1.5.1", 2000);
        assert_eq!(read(&log, &build), None);
    }

    #[test]
    fn logs_older_than_the_build_are_rejected_and_equal_times_qualify() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("Player.log");
        let previous = temp.path().join("Player-prev.log");
        let build = temp.path().join("GameAssembly.dll");
        write_at(&build, "fixture build", 2000);
        write_at(&log, "game version: 1.5.0", 1999);
        write_at(&previous, "game version: 1.5.1", 1998);
        assert_eq!(read(&log, &build), None);
        write_at(&previous, "game version: 1.5.1", 2000);
        assert_eq!(read(&log, &build).as_deref(), Some("1.5.1"));
        // An updated build invalidates both earlier logs.
        write_at(&build, "updated fixture build", 2001);
        assert_eq!(read(&log, &build), None);
    }

    #[test]
    fn player_log_is_preferred_and_previous_is_used_when_it_cannot_supply_a_version() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("Player.log");
        let previous = temp.path().join("Player-prev.log");
        let build = temp.path().join("GameAssembly.dll");
        write_at(&build, "fixture build", 2000);
        assert_eq!(read(&log, &build), None);
        write_at(&previous, "game version: 1.5.1", 2002);
        assert_eq!(read(&log, &build).as_deref(), Some("1.5.1"));
        write_at(&log, "game version: 1.5.2", 2001);
        // Player.log wins even if the previous log has a newer write time.
        assert_eq!(read(&log, &build).as_deref(), Some("1.5.2"));
        write_at(&log, "the game has not logged its version yet", 2001);
        assert_eq!(read(&log, &build).as_deref(), Some("1.5.1"));
        fs::remove_file(&log).unwrap();
        fs::create_dir(&log).unwrap();
        assert_eq!(read(&log, &build).as_deref(), Some("1.5.1"));
    }

    #[test]
    fn parsing_extracts_only_the_dotted_version() {
        assert_eq!(parse("Mono path\ngame version: 1.5.2. internal version: 9.9.9\n").as_deref(), Some("1.5.2"));
        assert_eq!(parse("no version here"), None);
        assert_eq!(parse("game version: unknown"), None);
    }
}
