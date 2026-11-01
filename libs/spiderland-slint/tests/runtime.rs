//! Background work and timers against a real Slint event-loop proxy. One test: Slint's platform is set once per process.

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};
use spiderland::{Effects, Feature, Host, Intents, Panicked, Store, Subscriptions};
use spiderland_slint::{Background, Dropping, every, local, on_wake};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use task_runtime::Runtime;
use tokio::sync::oneshot;

type UiEvent = Box<dyn FnOnce() + Send>;
static EVENTS: Mutex<VecDeque<UiEvent>> = Mutex::new(VecDeque::new());
static CLOCK_MS: AtomicU64 = AtomicU64::new(0);

struct Proxy;
impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        Ok(())
    }
    fn invoke_from_event_loop(&self, event: UiEvent) -> Result<(), slint::EventLoopError> {
        EVENTS.lock().unwrap().push_back(event);
        Ok(())
    }
}

struct TestPlatform(Rc<MinimalSoftwareWindow>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy))
    }
    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(CLOCK_MS.load(Ordering::SeqCst))
    }
}

fn pump() {
    slint::platform::update_timers_and_animations();
    loop {
        let event = EVENTS.lock().unwrap().pop_front();
        let Some(event) = event else {
            return;
        };
        event();
    }
}

fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "work must finish");
        std::thread::sleep(Duration::from_millis(1));
        pump();
    }
}

struct Probe;

enum Effect {
    Work(fn() -> i32),
}

enum Intent {
    Run(fn() -> i32),
    Finished(Result<i32, Panicked>),
    Watch(bool),
    Tick,
}

#[derive(Default)]
struct State {
    results: Vec<Result<i32, Panicked>>,
    watching: bool,
    ticks: i32,
}

impl Feature for Probe {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect>) {
        match intent {
            Intent::Run(work) => effects.push(Effect::Work(work)),
            Intent::Finished(result) => state.results.push(result),
            Intent::Watch(on) => state.watching = on,
            Intent::Tick => state.ticks += 1,
        }
    }
}

struct Seen {
    results: Vec<Result<i32, Panicked>>,
    ticks: i32,
}

struct Runner {
    background: Background,
    seen: Rc<RefCell<Seen>>,
    watch: Subscriptions<()>,
}

impl Host<Probe> for Runner {
    fn render(&mut self, state: &State, intents: &Intents<Intent>) {
        let mut seen = self.seen.borrow_mut();
        seen.results.clone_from(&state.results);
        seen.ticks = state.ticks;
        self.watch
            .sync(state.watching.then_some(()), |()| every(Duration::from_millis(100), intents, || Intent::Tick));
    }

    fn perform(&mut self, effect: Effect, intents: &Intents<Intent>) {
        let Effect::Work(work) = effect;
        self.background.run(intents, work, Intent::Finished);
    }
}

#[test]
fn work_and_timers_come_back_as_intents_on_the_ui_thread() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window))).unwrap();

    let runtime = Runtime::start().unwrap();
    let background = Background::new(runtime.tasks().scope());
    let seen = Rc::new(RefCell::new(Seen { results: Vec::new(), ticks: 0 }));
    let store = Store::new(
        State::default(),
        Runner {
            background: background.clone(),
            seen: seen.clone(),
            watch: Subscriptions::default(),
        },
    );

    store.send(Intent::Run(|| 7));
    assert_eq!(background.pending(), 1, "the work's result has yet to come back");
    until(|| background.pending() == 0);
    assert_eq!(seen.borrow().results, [Ok(7)]);

    // A panic reaches the feature as a value, and the store goes on working.
    store.send(Intent::Run(|| panic!("a fault in the work, expected by this test")));
    until(|| background.pending() == 0);
    assert_eq!(seen.borrow().results, [Ok(7), Err(Panicked)]);
    store.send(Intent::Run(|| 8));
    until(|| background.pending() == 0);
    assert_eq!(seen.borrow().results, [Ok(7), Err(Panicked), Ok(8)]);

    // A timer runs only while the state wants it.
    let tick = |by: u64| {
        CLOCK_MS.fetch_add(by, Ordering::SeqCst);
        pump();
    };
    store.send(Intent::Watch(true));
    tick(100);
    tick(100);
    assert_eq!(seen.borrow().ticks, 2);
    store.send(Intent::Watch(false));
    tick(1000);
    assert_eq!(seen.borrow().ticks, 2, "no tick once the state no longer wants the timer");

    // A wake from another thread arrives on the UI thread, and a burst of them makes at most two intents.
    let (waker, listening) = on_wake(&store.intents(), || Intent::Tick);
    pump(); // The listener starts waiting.
    let from_afar = waker.clone();
    std::thread::spawn(move || {
        for _ in 0..100 {
            from_afar.wake();
        }
    })
    .join()
    .unwrap();
    until(|| seen.borrow().ticks > 2);
    pump();
    let after_burst = seen.borrow().ticks;
    assert!((3..=4).contains(&after_burst), "a hundred wakes before it ran made {} intents", after_burst - 2);
    drop(listening);
    pump();
    waker.wake();
    pump();
    assert_eq!(seen.borrow().ticks, after_burst, "a wake after the guard is dropped goes nowhere");

    // Work waited for on the UI thread hands over its result, and its guard says it is done.
    let (answer, answered) = oneshot::channel();
    let got = Rc::new(RefCell::new(None));
    let taken = got.clone();
    let finished = local(answered, move |result: Result<i32, _>| *taken.borrow_mut() = Some(result.unwrap()));
    pump();
    assert!(!finished.is_finished());
    answer.send(5).unwrap();
    until(|| got.borrow().is_some());
    assert_eq!(*got.borrow(), Some(5));
    assert!(finished.is_finished());

    // Dropping the guard drops the work, whose owner can tell, and its result goes nowhere.
    let (answer, answered) = oneshot::channel::<i32>();
    let got = Rc::new(RefCell::new(None));
    let taken = got.clone();
    let dropped = local(answered, move |result| *taken.borrow_mut() = Some(result.unwrap()));
    pump();
    drop(dropped);
    until(|| answer.is_closed());
    assert!(got.borrow().is_none());

    // What its own result causes can let go of the work, as a reducer does: nothing wedges.
    let (answer, answered) = oneshot::channel::<i32>();
    let guard: Rc<RefCell<Option<Dropping>>> = Rc::default();
    let held = guard.clone();
    let done = Rc::new(RefCell::new(false));
    let saw = done.clone();
    *guard.borrow_mut() = Some(local(answered, move |_| {
        drop(held.borrow_mut().take());
        *saw.borrow_mut() = true;
    }));
    pump();
    answer.send(1).unwrap();
    until(|| *done.borrow());
    assert!(guard.borrow().is_none());

    assert!(runtime.shutdown(), "closing reports that accepted work panicked");
}
