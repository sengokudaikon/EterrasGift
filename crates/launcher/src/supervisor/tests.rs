use super::*;
use crate::process::Output;
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Start(String),
    Report(Stage, String),
    Detail(String),
    Say(String),
    Stop(String, Option<String>),
    Show,
    Hide,
    Exit(i32),
}

#[derive(Default)]
struct FakeSurface(Mutex<Vec<Event>>);

impl FakeSurface {
    fn events(&self) -> Vec<Event> {
        self.0.lock().unwrap().clone()
    }
    fn push(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

impl Surface for FakeSurface {
    fn start(&self, message: &str) {
        self.push(Event::Start(message.into()));
    }
    fn report(&self, stage: Stage, message: &str) {
        self.push(Event::Report(stage, message.into()));
    }
    fn detail(&self, line: &str) {
        self.push(Event::Detail(line.into()));
    }
    fn say(&self, message: &str) {
        self.push(Event::Say(message.into()));
    }
    fn stop(&self, reason: &str, message: Option<&str>) {
        self.push(Event::Stop(reason.into(), message.map(Into::into)));
    }
    fn show(&self) {
        self.push(Event::Show);
    }
    fn hide(&self) {
        self.push(Event::Hide);
    }
    fn exit(&self, code: i32) {
        self.push(Event::Exit(code));
    }
}

/// Hands out generations by key, or a failure, in order; never builds.
struct FakePreparer {
    results: Mutex<VecDeque<Result<&'static str, Failure>>>,
    directory: PathBuf,
}

impl Preparer for FakePreparer {
    fn prepare(&self, _root: &Path, _packaged: &Path, progress: crate::builder::Progress<'_>, output: Output<'_>, _cancel: &Cancellation) -> Result<Prepared, Failure> {
        progress(Stage::Checking, "Checking application components");
        output("   Compiling eterras-gift v0.1.0");
        let key = self.results.lock().unwrap().pop_front().expect("an expected preparation")?;
        Ok(Prepared {
            directory: self.directory.join(key),
            key: key.into(),
            built: true,
        })
    }
}

/// Answers each start with the next exit, and records what it was asked to run.
#[derive(Default)]
struct FakeApps {
    exits: Mutex<VecDeque<Result<Exit, Failure>>>,
    runs: Mutex<Vec<(PathBuf, Vec<String>)>>,
}

impl AppRunner for FakeApps {
    fn run(&self, executable: &Path, arguments: &[String], _root: &Path, started: &dyn Fn(), _cancel: &Cancellation) -> Result<Exit, Failure> {
        self.runs.lock().unwrap().push((executable.to_path_buf(), arguments.to_vec()));
        let exit = self.exits.lock().unwrap().pop_front().expect("an expected start");
        if exit.is_ok() {
            started();
        }
        exit
    }
}

const LONG: Duration = Duration::from_secs(60);
const QUICK: Duration = Duration::from_secs(1);

fn exit(code: i32, ran: Duration) -> Result<Exit, Failure> {
    Ok(Exit { code, ran })
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    runtime: PathBuf,
    supervisor: Supervisor<FakePreparer, FakeApps, FakeSurface>,
}

fn fixture(preparations: Vec<Result<&'static str, Failure>>, exits: Vec<Result<Exit, Failure>>) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let runtime = temp.path().join("runtime");
    let preparer = FakePreparer {
        results: Mutex::new(preparations.into()),
        directory: temp.path().join("generations"),
    };
    let apps = FakeApps {
        exits: Mutex::new(exits.into()),
        ..Default::default()
    };
    let supervisor = Supervisor::new(
        &root,
        &temp.path().join("launcher"),
        vec!["--settings".into()],
        &runtime,
        preparer,
        apps,
        FakeSurface::default(),
    );
    Fixture {
        _temp: temp,
        root,
        runtime,
        supervisor,
    }
}

impl Fixture {
    fn events(&self) -> Vec<Event> {
        self.supervisor.surface().events()
    }
    fn runs(&self) -> Vec<(PathBuf, Vec<String>)> {
        self.supervisor.apps.runs.lock().unwrap().clone()
    }
    /// The window's own state, without the detail lines.
    fn states(&self) -> Vec<Event> {
        self.events().into_iter().filter(|event| !matches!(event, Event::Detail(_))).collect()
    }
}

#[test]
fn an_app_that_exits_ends_the_launcher() {
    let f = fixture(vec![Ok("A")], vec![exit(0, LONG)]);
    f.supervisor.prepare_and_run();
    assert_eq!(
        f.states(),
        [
            Event::Start("Checking application components…".into()),
            Event::Report(Stage::Checking, "Checking application components".into()),
            Event::Hide,
            Event::Exit(0),
        ]
    );
    let details: Vec<_> = f
        .events()
        .into_iter()
        .filter_map(|event| if let Event::Detail(line) = event { Some(line) } else { None })
        .collect();
    assert_eq!(details, ["Checking: Checking application components", "   Compiling eterras-gift v0.1.0"]);
    let (executable, arguments) = &f.runs()[0];
    assert_eq!(executable, &f.supervisor.preparer.directory.join("A").join(paths::EXECUTABLE));
    let root = f.root.display().to_string();
    assert_eq!(arguments, &["--settings", "--root", &root, "--supervised", "--generation", "A"]);
    let log = fs::read_to_string(f.runtime.join("preparation.log")).unwrap();
    assert!(log.contains("ms Checking: Checking application components") && log.contains("Compiling"), "{log}");
}

#[test]
fn a_restart_request_prepares_again_and_starts_the_new_generation() {
    let f = fixture(vec![Ok("A"), Ok("B")], vec![exit(REQUESTED_EXIT_CODE, LONG), exit(0, LONG)]);
    f.supervisor.prepare_and_run();
    let states = f.states();
    let restarting = states
        .iter()
        .position(|event| *event == Event::Report(Stage::Restarting, "Restarting Eterra's Gift…".into()))
        .unwrap();
    assert_eq!(states[restarting - 1], Event::Show, "the window returns while it prepares again");
    assert_eq!(states.last(), Some(&Event::Exit(0)));
    let runs = f.runs();
    assert_eq!(runs.len(), 2);
    assert!(runs[1].1.ends_with(&["--generation".into(), "B".into()]));
}

#[test]
fn a_quick_restart_counts_once_per_generation() {
    let f = fixture(vec![Ok("A"), Ok("A")], vec![exit(REQUESTED_EXIT_CODE, QUICK), exit(REQUESTED_EXIT_CODE, QUICK)]);
    f.supervisor.prepare_and_run();
    let message = "The application requested another restart for the same generation. Retry after checking the preparation log.";
    assert_eq!(f.runs().len(), 2, "one quick restart, then a stop");
    assert_eq!(
        f.states()[f.states().len() - 2..],
        [
            Event::Stop(message.into(), Some("Preparation stopped. Resolve the problem below, then Retry.".into())),
            Event::Show
        ]
    );
    assert!(
        fs::read_dir(&f.runtime)
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with("fault-")),
        "a fault report"
    );

    // A user's Restart now after a run is always taken, however often.
    let f = fixture(
        vec![Ok("A"), Ok("A"), Ok("A")],
        vec![exit(REQUESTED_EXIT_CODE, LONG), exit(REQUESTED_EXIT_CODE, LONG), exit(0, LONG)],
    );
    f.supervisor.prepare_and_run();
    assert_eq!(f.runs().len(), 3);
    assert_eq!(f.states().last(), Some(&Event::Exit(0)));
}

#[test]
fn a_fault_is_said_with_retry_and_never_restarts() {
    let f = fixture(vec![Ok("A"), Ok("A")], vec![exit(3, LONG), exit(0, LONG)]);
    f.supervisor.prepare_and_run();
    assert_eq!(f.runs().len(), 1, "no automatic start after a fault");
    assert_eq!(
        f.states()[f.states().len() - 2..],
        [
            Event::Stop(
                "Eterra's Gift stopped unexpectedly (exit code 3). Its fault report is in the logs folder of its data folder.".into(),
                Some("Eterra's Gift stopped. Retry starts it again.".into())
            ),
            Event::Show
        ]
    );
    assert!(f.events().contains(&Event::Detail("Eterra's Gift stopped (exit code 3).".into())));
    // Retry starts it again.
    f.supervisor.prepare_and_run();
    assert_eq!(f.runs().len(), 2);
    assert_eq!(f.states().last(), Some(&Event::Exit(0)));
}

#[test]
fn a_failed_preparation_stops_with_its_reason() {
    let f = fixture(vec![Err(Failure::Failed("cargo exited with 101.\nerror".into()))], vec![]);
    f.supervisor.prepare_and_run();
    assert!(f.runs().is_empty());
    assert_eq!(
        f.states()[f.states().len() - 2..],
        [
            Event::Stop(
                "cargo exited with 101.\nerror".into(),
                Some("Preparation stopped. Resolve the problem below, then Retry.".into())
            ),
            Event::Show
        ]
    );
}

#[test]
fn an_app_that_cannot_start_stops_preparation() {
    let f = fixture(vec![Ok("A")], vec![Err(Failure::Failed("Cannot launch Eterra's Gift. not found".into()))]);
    f.supervisor.prepare_and_run();
    assert!(matches!(&f.states()[f.states().len() - 2], Event::Stop(reason, _) if reason.starts_with("Cannot launch")));
}

#[test]
fn closing_cancels_and_ends_once() {
    let f = fixture(vec![Err(Failure::Cancelled)], vec![]);
    f.supervisor.close();
    f.supervisor.prepare_and_run();
    assert_eq!(f.states().iter().filter(|event| matches!(event, Event::Exit(_))).count(), 1);
    assert!(!f.states().iter().any(|event| matches!(event, Event::Stop(..))), "a cancelled preparation is no failure");
}

#[test]
fn settings_waits_for_a_build_and_its_restart_prepares_again() {
    let f = fixture(vec![Ok("A")], vec![exit(REQUESTED_EXIT_CODE, LONG), exit(0, LONG)]);
    f.supervisor.open_settings();
    assert_eq!(
        f.states(),
        [Event::Say(
            "Settings will be available after the application builds. Check the preparation details, then Retry.".into()
        )]
    );
    assert!(f.runs().is_empty());

    // The newest build there is: the launcher's own folder holds one.
    let launcher = f.supervisor.packaged.clone();
    fs::create_dir_all(&launcher).unwrap();
    fs::write(launcher.join(paths::EXECUTABLE), "").unwrap();
    f.supervisor.open_settings();
    let (executable, arguments) = &f.runs()[0];
    assert_eq!(executable, &launcher.join(paths::EXECUTABLE));
    assert!(arguments.ends_with(&["--supervised".into(), "--settings".into(), "--no-connect".into()]));
    assert!(!arguments.contains(&"--generation".into()));
    // It exited asking for a restart: preparation starts, and runs the app.
    assert_eq!(f.runs().len(), 2);
    assert_eq!(f.states().last(), Some(&Event::Exit(0)));
}

#[test]
fn the_previous_log_is_kept_beside_the_current_one() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("preparation.log");
    start_log(&log);
    fs::write(&log, "first").unwrap();
    start_log(&log);
    fs::write(&log, "second").unwrap();
    start_log(&log);
    assert_eq!(fs::read_to_string(temp.path().join("preparation.previous.log")).unwrap(), "second");
    assert!(!log.exists());
}
