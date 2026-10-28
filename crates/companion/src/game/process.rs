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
#[cfg(windows)]
pub fn oldest_running(process_name: &str) -> Option<RunningGame> {
    let wanted = format!("{process_name}.exe");
    let mut oldest: Option<RunningGame> = None;
    eg_core::process::each(|process_id, executable| {
        if eg_core::process::is_named(executable, &wanted)
            && let Some(start_time) = start_time(process_id)
            && oldest.is_none_or(|known| start_time < known.start_time)
        {
            oldest = Some(RunningGame { process_id, start_time });
        }
    })
    .ok()?;
    oldest
}

/// When the process started, or None when it has exited or can't be queried.
#[cfg(windows)]
fn start_time(process_id: u32) -> Option<Timestamp> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: a query-only handle, closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    let mut exit_code = 0u32;
    // SAFETY: `process` is open with the rights both calls need.
    let known = unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) } != 0
        && unsafe { GetExitCodeProcess(process, &mut exit_code) } != 0
        && exit_code == STILL_ACTIVE as u32;
    // SAFETY: the handle is ours.
    unsafe { CloseHandle(process) };
    known.then(|| from_file_time(created)).flatten()
}

/// FILETIME counts 100 ns since 1601.
#[cfg(windows)]
fn from_file_time(time: windows_sys::Win32::Foundation::FILETIME) -> Option<Timestamp> {
    let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    eg_core::time::from_filetime(ticks).ok()
}

#[cfg(not(windows))]
pub fn oldest_running(_process_name: &str) -> Option<RunningGame> {
    None
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
