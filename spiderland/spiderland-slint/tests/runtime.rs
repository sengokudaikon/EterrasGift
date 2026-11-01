//! Background work and timers against a real Slint event-loop proxy. One test: Slint's platform is set once per process.

use spiderland::{Effects, Feature, Host, Intents, Panicked, Store, Subscriptions};
use spiderland_slint::testing::{Headless, advance, settle, until};
use spiderland_slint::{Background, Dropping, Listening, after, every, finish, local, on_wake, on_watch};
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{oneshot, watch};

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
    let _headless = Headless::frozen();

    let panics = Arc::new(AtomicUsize::new(0));
    let counted = panics.clone();
    let background = Background::new(move |job| {
        let counted = counted.clone();
        std::thread::spawn(move || {
            if catch_unwind(AssertUnwindSafe(job)).is_err() {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });
        true
    });
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
    until("background work", || background.pending() == 0);
    assert_eq!(seen.borrow().results, [Ok(7)]);

    // A panic reaches the feature as a value, and the store goes on working.
    store.send(Intent::Run(|| panic!("a fault in the work, expected by this test")));
    until("background work", || background.pending() == 0);
    assert_eq!(seen.borrow().results, [Ok(7), Err(Panicked)]);
    until("the panic to reach the pool", || panics.load(Ordering::SeqCst) == 1);
    store.send(Intent::Run(|| 8));
    until("background work", || background.pending() == 0);
    assert_eq!(seen.borrow().results, [Ok(7), Err(Panicked), Ok(8)]);

    // Work the pool refuses, or lets go of before it starts, sends nothing back.
    let intents = store.intents();
    let refusing = Background::new(|_| false);
    refusing.run(&intents, || 1, Intent::Finished);
    assert_eq!(refusing.pending(), 0);
    let letting_go = Background::new(|job| {
        drop(job);
        true
    });
    letting_go.run(&intents, || 2, Intent::Finished);
    until("the job let go of", || letting_go.pending() == 0);
    settle();
    assert_eq!(seen.borrow().results, [Ok(7), Err(Panicked), Ok(8)]);

    // An answer awaited on the UI thread counts until it is delivered, and comes back as an intent.
    let (answer, answered) = oneshot::channel();
    background.wait(&store.intents(), answered, |result: Result<i32, _>| Intent::Finished(Ok(result.unwrap())));
    settle();
    assert_eq!(background.pending(), 1, "the answer has yet to come");
    answer.send(9).unwrap();
    until("background work", || background.pending() == 0);
    assert_eq!(seen.borrow().results.last(), Some(&Ok(9)));

    // A timer runs only while the state wants it.
    let tick = |by: u64| advance(Duration::from_millis(by));
    store.send(Intent::Watch(true));
    tick(100);
    tick(100);
    assert_eq!(seen.borrow().ticks, 2);
    store.send(Intent::Watch(false));
    tick(1000);
    assert_eq!(seen.borrow().ticks, 2, "no tick once the state no longer wants the timer");

    // A wake from another thread arrives on the UI thread, and a burst of them makes at most two intents.
    let (waker, listening) = on_wake(&store.intents(), || Intent::Tick);
    settle(); // The listener starts waiting.
    let from_afar = waker.clone();
    std::thread::spawn(move || {
        for _ in 0..100 {
            from_afar.wake();
        }
    })
    .join()
    .unwrap();
    until("the wakes", || seen.borrow().ticks > 2);
    settle();
    let after_burst = seen.borrow().ticks;
    assert!((3..=4).contains(&after_burst), "a hundred wakes before it ran made {} intents", after_burst - 2);
    drop(listening);
    settle();
    waker.wake();
    settle();
    assert_eq!(seen.borrow().ticks, after_burst, "a wake after the guard is dropped goes nowhere");

    // Dropping the guard lets go of what the listener holds at once, with nothing published to wake it.
    let held = Rc::new(());
    let holding = held.clone();
    let (_publisher, updates) = watch::channel(0);
    let listening = on_watch(updates, &store.intents(), move || {
        let _ = &holding;
        Intent::Tick
    });
    settle();
    assert_eq!(Rc::strong_count(&held), 2);
    drop(listening);
    settle();
    assert_eq!(Rc::strong_count(&held), 1, "a dropped listener must not keep what it holds");

    // What a listener's own intent causes can let go of it, as a reducer's state does: a value published meanwhile isn't heard.
    let (publisher, updates) = watch::channel(0);
    let guard: Rc<RefCell<Option<Listening>>> = Rc::default();
    let (heard, held, again) = (Rc::new(Cell::new(0)), guard.clone(), publisher.clone());
    let counting = heard.clone();
    *guard.borrow_mut() = Some(on_watch(updates, &store.intents(), move || {
        counting.set(counting.get() + 1);
        again.send_replace(2);
        drop(held.borrow_mut().take());
        Intent::Tick
    }));
    settle();
    publisher.send_replace(1);
    until("the listener's own intent", || heard.get() > 0);
    settle();
    assert_eq!(heard.get(), 1, "nothing is heard after the guard is gone");
    assert!(guard.borrow().is_none());

    // A delayed intent fires once, and the timer that replaces it takes its place.
    let ticks = seen.borrow().ticks;
    let intents = store.intents();
    let replaced = after(Duration::from_millis(100), &intents, || Intent::Tick);
    tick(50);
    drop(replaced);
    let delayed = after(Duration::from_millis(100), &intents, || Intent::Tick);
    tick(50);
    assert_eq!(seen.borrow().ticks, ticks, "the first was replaced, and the second isn't due");
    tick(50);
    assert_eq!(seen.borrow().ticks, ticks + 1);
    tick(500);
    assert_eq!(seen.borrow().ticks, ticks + 1, "it fires once");
    drop(delayed);

    // Work waited for on the UI thread hands over its result, and its guard says it is done.
    let (answer, answered) = oneshot::channel();
    let got = Rc::new(RefCell::new(None));
    let taken = got.clone();
    let finished = local(answered, move |result: Result<i32, _>| *taken.borrow_mut() = Some(result.unwrap()));
    settle();
    assert!(!finished.is_finished());
    answer.send(5).unwrap();
    until("the answer", || got.borrow().is_some());
    assert_eq!(*got.borrow(), Some(5));
    assert!(finished.is_finished());

    // Dropping the guard drops the work, whose owner can tell, and its result goes nowhere.
    let (answer, answered) = oneshot::channel::<i32>();
    let got = Rc::new(RefCell::new(None));
    let taken = got.clone();
    let dropped = local(answered, move |result| *taken.borrow_mut() = Some(result.unwrap()));
    settle();
    drop(dropped);
    until("letting go", || answer.is_closed());
    assert!(got.borrow().is_none());

    // Work that must be heard hands over its result with no guard to drop.
    let (answer, answered) = oneshot::channel();
    let got = Rc::new(RefCell::new(None));
    let taken = got.clone();
    finish(answered, move |result: Result<i32, _>| *taken.borrow_mut() = Some(result.unwrap()));
    settle();
    answer.send(6).unwrap();
    until("the answer", || got.borrow().is_some());
    assert_eq!(*got.borrow(), Some(6));

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
    settle();
    answer.send(1).unwrap();
    until("its result", || *done.borrow());
    assert!(guard.borrow().is_none());
}
