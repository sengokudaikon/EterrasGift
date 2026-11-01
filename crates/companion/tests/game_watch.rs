use eg_companion::game::install::GameInstall;
use eg_companion::game::process::RunningGame;
use eg_companion::game::watch::{GameSeen, GameWatch};
use eg_companion::launch::host::{GameHost, LaunchError, LaunchMode, LaunchOutcome};
use eg_companion::launch::launcher::GameLauncher;
use eg_companion::launch::loader::ModLoaderState;
use eg_core::bloom::BloomPhase;
use eg_core::game_log::{GameLogState, GameReadiness};
use eg_core::operations::Cancellation;
use eg_runtime::Runtime;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tokio::sync::watch;

const OFTEN: Duration = Duration::from_millis(5);
/// Long enough that a test only sees a look it asked for.
const RARELY: Duration = Duration::from_secs(600);
const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Host {
    running: Mutex<Option<RunningGame>>,
    readiness: Mutex<GameReadiness>,
    looks: AtomicUsize,
    overlapping: AtomicBool,
    inside: AtomicUsize,
    loader_reads: AtomicUsize,
    panic_next: AtomicBool,
    /// Holds each look until told to go on.
    gate: Mutex<Option<mpsc::Receiver<()>>>,
}

impl GameHost for Host {
    fn find_install(&self) -> Option<GameInstall> {
        None
    }
    fn find_running(&self) -> Option<RunningGame> {
        if self.inside.fetch_add(1, Ordering::SeqCst) > 0 {
            self.overlapping.store(true, Ordering::SeqCst);
        }
        if let Some(gate) = self.gate.lock().unwrap().as_ref() {
            gate.recv_timeout(PATIENCE).unwrap();
        }
        self.inside.fetch_sub(1, Ordering::SeqCst);
        self.looks.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic_next.swap(false, Ordering::SeqCst), "fixture panic");
        *self.running.lock().unwrap()
    }
    fn read_log(&self, _: &RunningGame) -> GameLogState {
        GameLogState {
            readiness: *self.readiness.lock().unwrap(),
            ..GameLogState::started()
        }
    }
    fn read_mod_loader(&self, _: Option<&GameInstall>) -> ModLoaderState {
        self.loader_reads.fetch_add(1, Ordering::SeqCst);
        ModLoaderState::SetAside
    }
    fn loader_root(&self, _: &GameInstall) -> PathBuf {
        unreachable!("the watch does not access loader folders")
    }
    fn read_game_version(&self, _: &GameInstall) -> Option<String> {
        None
    }
    fn launch(&self, _: LaunchMode, _: Option<&GameInstall>, _: &Cancellation) -> Result<LaunchOutcome, LaunchError> {
        unreachable!("the watch starts nothing")
    }
    fn settle_mod_loader(&self, _: Option<&GameInstall>) -> bool {
        false
    }
}

fn fixture() -> (Arc<Host>, Arc<GameLauncher>, Runtime) {
    let host = Arc::new(Host::default());
    let launcher = Arc::new(GameLauncher::new(host.clone()));
    (host, launcher, Runtime::start().unwrap())
}

fn game() -> RunningGame {
    RunningGame {
        process_id: 7,
        start_time: jiff::Timestamp::UNIX_EPOCH,
    }
}

fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "it must happen in time");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn next(tasks: &eg_runtime::Tasks, updates: &mut watch::Receiver<GameSeen>) -> GameSeen {
    tasks.block_on(async {
        tokio::time::timeout(PATIENCE, updates.changed()).await.unwrap().unwrap();
    });
    *updates.borrow_and_update()
}

#[test]
fn it_looks_only_while_someone_follows() {
    let (host, launcher, runtime) = fixture();
    let watch = GameWatch::every(OFTEN, launcher, &runtime.tasks());
    std::thread::sleep(OFTEN * 10);
    assert_eq!(host.looks.load(Ordering::SeqCst), 0, "nobody follows, so nothing looks");
    let _updates = watch.subscribe();
    std::thread::sleep(OFTEN * 10);
    assert_eq!(host.looks.load(Ordering::SeqCst), 0, "subscribing alone asks for nothing");

    let following = watch.follow();
    until(|| host.looks.load(Ordering::SeqCst) >= 3);
    drop(following);
    // One already under way may finish.
    std::thread::sleep(OFTEN * 4);
    let settled = host.looks.load(Ordering::SeqCst);
    std::thread::sleep(OFTEN * 10);
    assert_eq!(host.looks.load(Ordering::SeqCst), settled, "the last follower gone, it stops");

    let again = watch.follow();
    until(|| host.looks.load(Ordering::SeqCst) > settled);
    drop(again);
    assert!(!runtime.shutdown());
}

#[test]
fn a_look_that_finds_the_same_wakes_nobody() {
    let (host, launcher, runtime) = fixture();
    let tasks = runtime.tasks();
    let watch = GameWatch::every(OFTEN, launcher, &tasks);
    let mut updates = watch.subscribe();
    let _following = watch.follow();
    until(|| host.looks.load(Ordering::SeqCst) >= 5);
    assert!(!updates.has_changed().unwrap(), "five looks at no game, and nothing changed");

    *host.running.lock().unwrap() = Some(game());
    *host.readiness.lock().unwrap() = GameReadiness::Online;
    let seen = next(&tasks, &mut updates);
    assert_eq!(seen.running, Some(game()));
    assert_eq!(seen.phase(), BloomPhase::Bloom);

    *host.running.lock().unwrap() = None;
    assert_eq!(next(&tasks, &mut updates).phase(), BloomPhase::Wilted);
    assert!(!runtime.shutdown());
}

#[test]
fn refresh_reads_the_loader_and_is_heard_with_nobody_following() {
    let (host, launcher, runtime) = fixture();
    let tasks = runtime.tasks();
    let watch = GameWatch::every(RARELY, launcher, &tasks);
    let mut updates = watch.subscribe();
    watch.refresh();
    assert_eq!(next(&tasks, &mut updates).loader, Some(ModLoaderState::SetAside));
    assert_eq!(host.loader_reads.load(Ordering::SeqCst), 1);

    // Nothing has changed, but a page coming into view still hears it.
    watch.refresh();
    assert_eq!(next(&tasks, &mut updates).loader, Some(ModLoaderState::SetAside));
    assert_eq!(host.loader_reads.load(Ordering::SeqCst), 2);
    assert_eq!(host.looks.load(Ordering::SeqCst), 2, "and it doesn't go on looking");
    assert!(!runtime.shutdown());
}

#[test]
fn an_ordinary_look_keeps_the_last_loader_it_read() {
    let (host, launcher, runtime) = fixture();
    let tasks = runtime.tasks();
    let watch = GameWatch::every(OFTEN, launcher, &tasks);
    let mut updates = watch.subscribe();
    let _following = watch.follow();
    watch.refresh();
    next(&tasks, &mut updates);
    let reads = host.loader_reads.load(Ordering::SeqCst);
    let looks = host.looks.load(Ordering::SeqCst);
    until(|| host.looks.load(Ordering::SeqCst) >= looks + 3);
    assert_eq!(host.loader_reads.load(Ordering::SeqCst), reads, "ordinary looks don't read it");
    assert_eq!(watch.current().loader, Some(ModLoaderState::SetAside));
    assert!(!runtime.shutdown());
}

#[test]
fn looks_never_overlap_and_requests_during_one_make_one_more() {
    let (host, launcher, runtime) = fixture();
    let (release, gate) = mpsc::channel();
    *host.gate.lock().unwrap() = Some(gate);
    let tasks = runtime.tasks();
    let watch = GameWatch::every(RARELY, launcher, &tasks);
    let mut updates = watch.subscribe();
    watch.refresh();
    until(|| host.inside.load(Ordering::SeqCst) == 1);
    for _ in 0..5 {
        watch.refresh();
    }
    release.send(()).unwrap();
    release.send(()).unwrap();
    next(&tasks, &mut updates);
    until(|| host.looks.load(Ordering::SeqCst) == 2);
    std::thread::sleep(OFTEN * 10);
    assert_eq!(host.looks.load(Ordering::SeqCst), 2);
    assert!(!host.overlapping.load(Ordering::SeqCst));
    drop(release);
    *host.gate.lock().unwrap() = None;
    assert!(!runtime.shutdown());
}

#[test]
fn a_panicking_look_is_seen_and_the_next_one_recovers() {
    let (host, launcher, runtime) = fixture();
    let tasks = runtime.tasks();
    let watch = GameWatch::every(RARELY, launcher, &tasks);
    let mut updates = watch.subscribe();
    host.panic_next.store(true, Ordering::SeqCst);
    watch.refresh();
    assert!(next(&tasks, &mut updates).failed);
    watch.refresh();
    assert!(!next(&tasks, &mut updates).failed);
    assert!(runtime.shutdown(), "the app's exit code still hears of it");
}

#[test]
fn it_ends_with_its_last_clone_and_with_the_runtime() {
    let (host, launcher, runtime) = fixture();
    let tasks = runtime.tasks();
    let watch = GameWatch::every(OFTEN, launcher.clone(), &tasks);
    let other = watch.clone();
    let mut updates = watch.subscribe();
    let following = watch.follow();
    until(|| host.looks.load(Ordering::SeqCst) >= 1);
    drop(watch);
    drop(following);
    drop(other);
    tasks.block_on(async {
        let ended = tokio::time::timeout(PATIENCE, async { while updates.changed().await.is_ok() {} });
        ended.await.unwrap();
    });

    let open = GameWatch::every(OFTEN, launcher, &tasks);
    let _following = open.follow();
    assert!(!runtime.shutdown(), "a watch nobody closed doesn't hold the runtime open");
    open.refresh();
}

#[test]
fn inline_looks_before_refresh_returns() {
    let (host, launcher, runtime) = fixture();
    *host.running.lock().unwrap() = Some(game());
    *host.readiness.lock().unwrap() = GameReadiness::NoCharacter;
    let watch = GameWatch::inline(launcher);
    let mut updates = watch.subscribe();
    let _following = watch.follow();
    assert_eq!(host.looks.load(Ordering::SeqCst), 0);
    watch.refresh();
    assert!(updates.has_changed().unwrap());
    let seen = *updates.borrow_and_update();
    assert_eq!(seen.phase(), BloomPhase::Opening);
    assert_eq!(seen.loader, Some(ModLoaderState::SetAside));
    assert!(!runtime.shutdown());
}
