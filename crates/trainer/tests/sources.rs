use eg_core::game_log::{GameLogStatus, GameReadiness};
use eg_trainer::{backend::ProcessIdentity, connect::admission::LogFactory, connect::sources::FileLogFactory};
use std::fs;

fn target() -> ProcessIdentity {
    ProcessIdentity {
        pid: 42,
        created: 134367912000000000,
        executable: std::env::temp_dir().join("authored-game.exe"),
    }
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
