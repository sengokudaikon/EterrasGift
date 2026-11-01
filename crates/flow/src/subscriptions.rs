//! Things that run for as long as the state wants them, such as a timer while a page shows.

use std::any::Any;

/// Running things by key. A render says which keys the state wants: new ones start, and ones no longer wanted are
/// dropped, which is what stops them. Deriving this from state means leaving a page and coming back can't leave two
/// timers running, as re-arming a timer from its own tick can.
pub struct Subscriptions<K> {
    live: Vec<(K, Box<dyn Any>)>,
}

impl<K> Default for Subscriptions<K> {
    fn default() -> Self {
        Subscriptions { live: Vec::new() }
    }
}

impl<K: PartialEq> Subscriptions<K> {
    /// `start` returns a guard for what it started: dropping the guard must stop it.
    pub fn sync<G: 'static>(&mut self, wanted: impl IntoIterator<Item = K>, mut start: impl FnMut(&K) -> G) {
        let wanted: Vec<K> = wanted.into_iter().collect();
        self.live.retain(|(key, _)| wanted.contains(key));
        for key in wanted {
            if !self.live.iter().any(|(live, _)| *live == key) {
                let guard = start(&key);
                self.live.push((key, Box::new(guard)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Guard(Rc<Cell<i32>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    #[test]
    fn what_is_wanted_runs_once_and_stops_when_no_longer_wanted() {
        let running = Rc::new(Cell::new(0));
        let start = |_: &&str| {
            running.set(running.get() + 1);
            Guard(running.clone())
        };
        let mut subscriptions = Subscriptions::default();
        subscriptions.sync(["watch"], start);
        subscriptions.sync(["watch"], start);
        assert_eq!(running.get(), 1, "a key still wanted keeps its one instance");
        subscriptions.sync([], start);
        assert_eq!(running.get(), 0, "a key no longer wanted stops");
        subscriptions.sync(["watch", "other"], start);
        assert_eq!(running.get(), 2);
    }
}
