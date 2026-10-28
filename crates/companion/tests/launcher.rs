use eg_companion::game::install::GameInstall;
use eg_companion::game::process::RunningGame;
use eg_companion::launch::host::{GameHost, LaunchError, LaunchMode, LaunchOutcome};
use eg_companion::launch::launcher::GameLauncher;
use eg_companion::launch::loader::ModLoaderState;
use eg_core::game_log::GameLogState;
use eg_core::operations::Cancellation;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

type Launch = dyn Fn(&Cancellation) -> Result<LaunchOutcome, LaunchError> + Send + Sync;

struct FakeGameHost {
    panic_on_find: AtomicBool,
    launch: Box<Launch>,
}

impl FakeGameHost {
    fn new(launch: impl Fn(&Cancellation) -> Result<LaunchOutcome, LaunchError> + Send + Sync + 'static) -> Self {
        Self {
            panic_on_find: AtomicBool::new(false),
            launch: Box::new(launch),
        }
    }
}

impl GameHost for FakeGameHost {
    fn find_install(&self) -> Option<GameInstall> {
        assert!(!self.panic_on_find.swap(false, Ordering::SeqCst), "fake discovery panic");
        None
    }

    fn find_running(&self) -> Option<RunningGame> {
        None
    }

    fn read_log(&self, _: &RunningGame) -> GameLogState {
        unreachable!("launcher tests do not read the game log")
    }

    fn read_mod_loader(&self, _: Option<&GameInstall>) -> ModLoaderState {
        ModLoaderState::NotInstalled
    }

    fn loader_root(&self, _: &GameInstall) -> PathBuf {
        unreachable!("launcher tests do not access loader folders")
    }

    fn read_game_version(&self, _: &GameInstall) -> Option<String> {
        None
    }

    fn launch(&self, _: LaunchMode, _: Option<&GameInstall>, lifetime: &Cancellation) -> Result<LaunchOutcome, LaunchError> {
        (self.launch)(lifetime)
    }

    fn settle_mod_loader(&self, _: Option<&GameInstall>) -> bool {
        false
    }
}

#[test]
fn a_host_panic_clears_starting_and_allows_another_start() {
    for discovery_panics in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let launched = calls.clone();
        let host = Arc::new(FakeGameHost::new(move |_| {
            let call = launched.fetch_add(1, Ordering::SeqCst);
            assert!(discovery_panics || call > 0, "fake launch panic");
            Ok(LaunchOutcome::Started)
        }));
        host.panic_on_find.store(discovery_panics, Ordering::SeqCst);
        let launcher = GameLauncher::new(host);

        assert!(catch_unwind(AssertUnwindSafe(|| launcher.play(LaunchMode::Offline))).is_err());
        assert!(!launcher.is_starting());
        assert!(launcher.play(LaunchMode::Offline).started);
        assert!(!launcher.is_starting());
        assert_eq!(calls.load(Ordering::SeqCst), if discovery_panics { 1 } else { 2 });
    }
}

#[test]
fn cancellation_before_a_start_never_calls_the_host() {
    let host = FakeGameHost::new(|_| panic!("a cancelled launcher must not launch"));
    host.panic_on_find.store(true, Ordering::SeqCst);
    let launcher = GameLauncher::new(Arc::new(host));

    launcher.cancel();
    launcher.cancel();
    let report = launcher.play(LaunchMode::OfflineWithMods);

    assert!(!report.started);
    assert_eq!(report.message, "The app is closing.");
    assert!(!launcher.is_starting());
    launcher.stop();
}

fn stop_during_start(host_panics: bool) {
    let (entered, active) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let launcher = Arc::new(GameLauncher::new(Arc::new(FakeGameHost::new(move |lifetime| {
        entered.send(lifetime.clone()).unwrap();
        released.lock().unwrap().recv_timeout(TIMEOUT).unwrap();
        assert!(!host_panics, "fake launch panic during shutdown");
        assert!(lifetime.is_cancelled());
        Err(LaunchError::Cancelled)
    }))));
    let playing = launcher.clone();
    let play = thread::spawn(move || catch_unwind(AssertUnwindSafe(|| playing.play(LaunchMode::OfflineWithMods))));
    let lifetime = active.recv_timeout(TIMEOUT).unwrap();
    assert!(launcher.is_starting());
    assert_eq!(launcher.play(LaunchMode::Offline).message, "Last Epoch is starting already.");

    if host_panics {
        // Cancellation requests cleanup without pretending the host has finished it.
        launcher.cancel();
        assert!(lifetime.is_cancelled());
        assert!(launcher.is_starting());
        assert_eq!(launcher.play(LaunchMode::Offline).message, "The app is closing.");
    }

    let (stopping, started_stop) = mpsc::channel();
    let (stopped, finished_stop) = mpsc::channel();
    let closing = launcher.clone();
    let stop = thread::spawn(move || {
        stopping.send(()).unwrap();
        closing.stop();
        stopped.send(()).unwrap();
    });
    started_stop.recv_timeout(TIMEOUT).unwrap();
    assert!(matches!(finished_stop.recv_timeout(Duration::from_millis(50)), Err(mpsc::RecvTimeoutError::Timeout)));
    assert!(lifetime.is_cancelled(), "stop must cancel the running host even without an earlier cancel call");
    release.send(()).unwrap();

    let result = play.join().unwrap();
    if host_panics {
        assert!(result.is_err());
    } else {
        let report = result.unwrap();
        assert!(!report.started);
        assert_eq!(report.message, "The app is closing.");
    }
    finished_stop.recv_timeout(TIMEOUT).expect("stop must wake after the host exits, including by panic");
    stop.join().unwrap();
    assert!(!launcher.is_starting());
    launcher.stop();
}

#[test]
fn stop_waits_for_a_cancelled_start_to_finish() {
    stop_during_start(false);
}

#[test]
fn a_panicking_start_releases_a_waiting_stop() {
    stop_during_start(true);
}
