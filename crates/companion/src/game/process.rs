//! The running game, found by name. A process is known by its id and start time only: the query that gives the start
//! time (`PROCESS_QUERY_LIMITED_INFORMATION`) reads nothing from the process.

use jiff::Timestamp;

/// A running game process, known by its id and start time only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunningGame {
    pub process_id: u32,
    pub start_time: Timestamp,
}

/// The oldest process named `process_name` (its executable without ".exe") that has not exited. A second instance of
/// the game exits by itself, so the oldest is the one that stays. Processes that exit mid-query, or that can't be
/// queried, are skipped.
pub fn oldest_running(process_name: &str) -> Option<RunningGame> {
    let (process_id, start_time) = win_platform::process::oldest_named(&format!("{process_name}.exe"))?;
    Some(RunningGame { process_id, start_time })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn this_test_process_is_found_by_its_name_and_start_time() {
        let executable = std::env::current_exe().unwrap();
        let name = executable.file_stem().unwrap().to_string_lossy().into_owned();
        let found = oldest_running(&name).unwrap();
        assert!(found.start_time <= Timestamp::now());
        assert!(found.start_time > Timestamp::now() - jiff::SignedDuration::from_mins(10));
        assert_eq!(oldest_running("no process is named this"), None);
    }
}
