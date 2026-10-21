//! The running game, found by name. A process is known by its id and start time only: the query that gives the start
//! time (`PROCESS_QUERY_LIMITED_INFORMATION`) reads nothing from the process (`spec/companion.md` §2).

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
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    let wanted = format!("{process_name}.exe").to_uppercase();
    // SAFETY: a snapshot of the process list; closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: PROCESSENTRY32W is plain data; dwSize is set as the API requires.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut oldest: Option<RunningGame> = None;
    // SAFETY: `snapshot` is open and `entry` is sized.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let length = entry.szExeFile.iter().position(|&unit| unit == 0).unwrap_or(entry.szExeFile.len());
        if String::from_utf16_lossy(&entry.szExeFile[..length]).to_uppercase() == wanted
            && let Some(start_time) = start_time(entry.th32ProcessID)
            && oldest.is_none_or(|known| start_time < known.start_time)
        {
            oldest = Some(RunningGame {
                process_id: entry.th32ProcessID,
                start_time,
            });
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot handle is ours.
    unsafe { CloseHandle(snapshot) };
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
