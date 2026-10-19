use eg_core::game_log::{GameLogStatus, GameReadiness};
use eg_trainer::{
    backend::{ProcessIdentity, ProcessLocator},
    connection::LogFactory,
    sources::{FileLogFactory, WatchProcessLocator},
};
use std::fs;

fn run<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
}
fn target() -> ProcessIdentity {
    ProcessIdentity {
        pid: 42,
        created: 134367912000000000,
        executable: std::env::temp_dir().join("authored-game.exe"),
    }
}

#[test]
fn published_locator_has_no_process_access_and_tracks_exact_observations_or_source_loss() {
    let (sender, receiver) = tokio::sync::watch::channel(vec![]);
    let mut locator = WatchProcessLocator::new(receiver);
    assert!(run(locator.find()).unwrap().is_empty());
    let first = target();
    sender.send_replace(vec![first.clone()]);
    assert_eq!(run(locator.find()).unwrap(), vec![first.clone()]);
    let reused = ProcessIdentity {
        created: first.created + 1,
        ..first.clone()
    };
    sender.send_replace(vec![first, reused.clone()]);
    assert_eq!(run(locator.find()).unwrap().len(), 2);
    sender.send_replace(vec![reused.clone()]);
    assert_eq!(run(locator.find()).unwrap(), vec![reused]);
    sender.send_replace(vec![]);
    assert!(run(locator.find()).unwrap().is_empty());
    drop(sender);
    assert!(run(locator.find()).is_err());
}

#[test]
fn file_log_factory_is_inert_and_produces_independent_process_bound_cursors() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("Player.log");
    let mut factory = FileLogFactory::new(&path).unwrap();
    assert!(!path.exists());
    assert_eq!(factory.path(), path);
    let mut first = factory.create(&target()).unwrap();
    let mut second = factory.create(&target()).unwrap();
    let started = "2026-10-19T10:00:00Z".parse().unwrap();
    assert_eq!(first.read(started).status, GameLogStatus::Missing);
    fs::write(&path,"2026-10-19T10:00:00Z\tINFO\tShellSceneManager: State transition SplashScreen -> CharacterSelect\n2026-10-19T10:00:01Z\tINFO\tLoading character: Fixture 1 isOnline: False, cycle: Legacy, class: Rogue\n").unwrap();
    assert_eq!(first.read(started).readiness, GameReadiness::Offline);
    assert_eq!(second.read(started).readiness, GameReadiness::Offline);
    assert_eq!(second.read("2026-10-20T10:00:00Z".parse().unwrap()).status, GameLogStatus::Stale);
    assert_eq!(first.read(started).readiness, GameReadiness::Offline);
    assert!(FileLogFactory::new("Player.log").is_err());
}
