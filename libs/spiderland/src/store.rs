//! The reducer's contract and the store that runs it.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

/// A feature's pure half. `reduce` touches nothing but its arguments, so it can be tested without a window or a worker.
pub trait Feature: 'static {
    /// Plain data: no handles and no live connections.
    type State: 'static;
    /// What happened: the user asked for something, or work came back.
    type Intent: 'static;
    /// Work to do outside the reducer, as data a test can compare.
    type Effect: 'static;

    fn reduce(state: &mut Self::State, intent: Self::Intent, effects: &mut Effects<Self::Effect>);
}

/// The effects one `reduce` asks for, performed in order after the next render.
pub struct Effects<E>(Vec<E>);

impl<E> Effects<E> {
    /// A list no one performs, for code shared with the reducer that only reads when it runs outside it, as a view does.
    pub fn detached() -> Self {
        Effects(Vec::new())
    }

    pub fn push(&mut self, effect: E) {
        self.0.push(effect);
    }

    /// Runs a child scope's reducer, which lists its own effect type, and wraps each of them as this feature's.
    pub fn scoped<C, R>(&mut self, wrap: impl Fn(C) -> E, child: impl FnOnce(&mut Effects<C>) -> R) -> R {
        let mut effects = Effects(Vec::new());
        let result = child(&mut effects);
        self.0.extend(effects.0.into_iter().map(wrap));
        result
    }
}

/// One reducer step, for tests: changes `state` and returns the effects `intent` asked for.
pub fn step<F: Feature>(state: &mut F::State, intent: F::Intent) -> Vec<F::Effect> {
    let mut effects = Effects(Vec::new());
    F::reduce(state, intent, &mut effects);
    effects.0
}

/// Background work panicked. Finished work hands this to its feature, which decides what the failure means there: a
/// fault in one feature's work is not automatically a fault in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panicked;

/// A feature's impure half: the view, the timers and the services.
pub trait Host<F: Feature>: 'static {
    /// Called once after each batch of intents. Should only do what the changed parts of the state need.
    fn render(&mut self, state: &F::State, intents: &Intents<F::Intent>);

    /// Starts one effect. Work that finishes later sends its result through `intents`.
    fn perform(&mut self, effect: F::Effect, intents: &Intents<F::Intent>);
}

trait Sink<I> {
    fn send(&self, intent: I);
}

/// Where intents go. It doesn't keep the store alive, so a timer or a thread holding one can't keep a feature going: once
/// the store is gone, sends do nothing.
pub struct Intents<I>(Weak<dyn Sink<I>>);

impl<I> Clone for Intents<I> {
    fn clone(&self) -> Self {
        Intents(self.0.clone())
    }
}

impl<I> Intents<I> {
    pub fn send(&self, intent: I) {
        if let Some(sink) = self.0.upgrade() {
            sink.send(intent);
        }
    }
}

struct Inner<F: Feature> {
    state: RefCell<F::State>,
    host: RefCell<Box<dyn Host<F>>>,
    queue: RefCell<VecDeque<F::Intent>>,
    draining: Cell<bool>,
    this: Weak<Inner<F>>,
}

/// Runs a feature on the UI thread. Clones share one store; whoever holds a clone keeps it alive (usually the view's
/// callbacks).
pub struct Store<F: Feature>(Rc<Inner<F>>);

impl<F: Feature> Clone for Store<F> {
    fn clone(&self) -> Self {
        Store(self.0.clone())
    }
}

impl<F: Feature> Store<F> {
    /// Starts the store and renders the first state.
    pub fn new(state: F::State, host: impl Host<F>) -> Self {
        let inner = Rc::new_cyclic(|this| Inner {
            state: RefCell::new(state),
            host: RefCell::new(Box::new(host) as Box<dyn Host<F>>),
            queue: RefCell::new(VecDeque::new()),
            draining: Cell::new(false),
            this: this.clone(),
        });
        let intents = inner.intents();
        inner.host.borrow_mut().render(&inner.state.borrow(), &intents);
        Store(inner)
    }

    /// Handles `intent` now, unless the store is already at work (a render's setter fired a callback, an effect finished
    /// at once): then it runs next, after the current batch. State is never borrowed twice.
    pub fn send(&self, intent: F::Intent) {
        Sink::send(&*self.0, intent);
    }

    pub fn intents(&self) -> Intents<F::Intent> {
        self.0.intents()
    }

    /// Runs `change` on the state directly, for a test or a capture to set up or look at a state. Nothing renders until the
    /// next intent. A real feature's changes go through its reducer.
    pub fn with_state<R>(&self, change: impl FnOnce(&mut F::State) -> R) -> R {
        change(&mut self.0.state.borrow_mut())
    }

    /// Answers a view's question from the state, for a pure callback that a binding calls. None while a reducer is running,
    /// which a binding can only meet if a reducer made the toolkit evaluate one.
    pub fn try_read<R>(&self, read: impl FnOnce(&F::State) -> R) -> Option<R> {
        self.0.state.try_borrow().ok().map(|state| read(&state))
    }
}

impl<F: Feature> Inner<F> {
    fn intents(&self) -> Intents<F::Intent> {
        let sink: Weak<dyn Sink<F::Intent>> = self.this.clone();
        Intents(sink)
    }

    fn drain(&self) {
        // Resets on a panic too, so a reducer that panics under a test harness doesn't wedge the store.
        struct Draining<'a>(&'a Cell<bool>);
        impl Drop for Draining<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        self.draining.set(true);
        let _draining = Draining(&self.draining);
        let intents = self.intents();
        loop {
            let mut effects = Effects(Vec::new());
            let mut any = false;
            loop {
                // The queue's borrow ends before the reducer runs.
                let next = self.queue.borrow_mut().pop_front();
                let Some(intent) = next else {
                    break;
                };
                F::reduce(&mut self.state.borrow_mut(), intent, &mut effects);
                any = true;
            }
            if !any {
                return;
            }
            let mut host = self.host.borrow_mut();
            host.render(&self.state.borrow(), &intents);
            for effect in effects.0 {
                host.perform(effect, &intents);
            }
        }
    }
}

impl<F: Feature> Sink<F::Intent> for Inner<F> {
    fn send(&self, intent: F::Intent) {
        self.queue.borrow_mut().push_back(intent);
        if !self.draining.get() {
            self.drain();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counter;

    #[derive(Debug, PartialEq)]
    enum Effect {
        Echo(i32),
    }

    enum Intent {
        Add(i32),
        AddLater(i32),
    }

    impl Feature for Counter {
        type State = i32;
        type Intent = Intent;
        type Effect = Effect;

        fn reduce(state: &mut i32, intent: Intent, effects: &mut Effects<Effect>) {
            match intent {
                Intent::Add(by) => *state += by,
                Intent::AddLater(by) => effects.push(Effect::Echo(by)),
            }
        }
    }

    #[derive(Default)]
    struct Log {
        renders: Vec<i32>,
        /// Sent from inside the next render, as a Slint setter firing a callback would.
        send_from_render: Option<i32>,
    }

    struct Recording(Rc<RefCell<Log>>);

    impl Host<Counter> for Recording {
        fn render(&mut self, state: &i32, intents: &Intents<Intent>) {
            let again = {
                let mut log = self.0.borrow_mut();
                log.renders.push(*state);
                log.send_from_render.take()
            };
            if let Some(by) = again {
                intents.send(Intent::Add(by));
            }
        }

        fn perform(&mut self, effect: Effect, intents: &Intents<Intent>) {
            let Effect::Echo(by) = effect;
            intents.send(Intent::Add(by));
        }
    }

    fn store() -> (Store<Counter>, Rc<RefCell<Log>>) {
        let log = Rc::new(RefCell::new(Log::default()));
        (Store::new(0, Recording(log.clone())), log)
    }

    #[test]
    fn it_renders_the_first_state_and_after_each_intent() {
        let (store, log) = store();
        store.send(Intent::Add(2));
        store.send(Intent::Add(3));
        assert_eq!(log.borrow().renders, [0, 2, 5]);
    }

    #[test]
    fn an_intent_sent_during_a_render_runs_next_without_a_second_borrow() {
        let (store, log) = store();
        log.borrow_mut().send_from_render = Some(10);
        store.send(Intent::Add(1));
        assert_eq!(log.borrow().renders, [0, 1, 11]);
    }

    #[test]
    fn an_effect_that_finishes_at_once_comes_back_as_an_intent() {
        let (store, log) = store();
        store.send(Intent::AddLater(7));
        assert_eq!(log.borrow().renders, [0, 0, 7]);
    }

    #[test]
    fn intents_do_nothing_once_the_store_is_gone() {
        let (store, log) = store();
        let intents = store.intents();
        drop(store);
        intents.send(Intent::Add(1));
        assert_eq!(log.borrow().renders, [0]);
    }

    #[test]
    fn a_panicking_reducer_does_not_wedge_the_store() {
        struct Fragile;
        impl Feature for Fragile {
            type State = i32;
            type Intent = bool;
            type Effect = ();
            fn reduce(state: &mut i32, panic_now: bool, _: &mut Effects<()>) {
                assert!(!panic_now);
                *state += 1;
            }
        }
        struct Quiet(Rc<Cell<i32>>);
        impl Host<Fragile> for Quiet {
            fn render(&mut self, state: &i32, _: &Intents<bool>) {
                self.0.set(*state);
            }
            fn perform(&mut self, _: (), _: &Intents<bool>) {}
        }
        let shown = Rc::new(Cell::new(-1));
        let store = Store::new(0, Quiet(shown.clone()));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.send(true)));
        assert!(panicked.is_err());
        store.send(false);
        assert_eq!(shown.get(), 1);
    }

    /// A scope is a state inside a state: the parent forwards the intent and maps the child's effects.
    #[test]
    fn a_child_scope_nests_by_forwarding_and_mapping() {
        struct Parent;
        #[derive(Debug, PartialEq)]
        enum ParentEffect {
            Child(Effect),
        }
        enum ParentIntent {
            Child(Intent),
            Close,
        }
        impl Feature for Parent {
            type State = Option<i32>;
            type Intent = ParentIntent;
            type Effect = ParentEffect;
            fn reduce(state: &mut Option<i32>, intent: ParentIntent, effects: &mut Effects<ParentEffect>) {
                match (intent, state.as_mut()) {
                    (ParentIntent::Child(intent), Some(child)) => effects.scoped(ParentEffect::Child, |effects| Counter::reduce(child, intent, effects)),
                    (ParentIntent::Child(_), None) => {}
                    (ParentIntent::Close, _) => *state = None,
                }
            }
        }
        let mut state = Some(1);
        assert_eq!(step::<Parent>(&mut state, ParentIntent::Child(Intent::AddLater(2))), [ParentEffect::Child(Effect::Echo(2))]);
        assert!(step::<Parent>(&mut state, ParentIntent::Child(Intent::Add(2))).is_empty());
        assert_eq!(state, Some(3));
        assert!(step::<Parent>(&mut state, ParentIntent::Close).is_empty());
        assert!(
            step::<Parent>(&mut state, ParentIntent::Child(Intent::AddLater(2))).is_empty(),
            "a closed scope takes no intents"
        );
    }
}
