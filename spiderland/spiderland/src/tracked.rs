//! Telling what changed without comparing it: Rust has no reactive properties, so state says when it was touched and a
//! render skips what depends only on untouched state.

use std::ops::{Deref, DerefMut};

/// How many times a [`Tracked`] value has been opened for writing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Version(u64);

/// A value whose version moves whenever it could have changed. Any `&mut` access counts, so a reducer can't change it and
/// forget to say so. Access that changes nothing still counts: at worst that redoes a render, and it never misses one.
#[derive(Clone, Debug, Default)]
pub struct Tracked<T> {
    value: T,
    version: Version,
}

impl<T> Tracked<T> {
    pub fn new(value: T) -> Self {
        Tracked { value, version: Version(0) }
    }

    pub fn version(&self) -> Version {
        self.version
    }
}

impl<T: PartialEq> Tracked<T> {
    /// Replaces the value, moving the version only if it differs: for a snapshot that often arrives unchanged.
    pub fn set(&mut self, value: T) {
        if self.value != value {
            **self = value;
        }
    }
}

impl<T> Deref for Tracked<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Tracked<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.version.0 += 1;
        &mut self.value
    }
}

/// One part of a render, redone only when its key changes. The key is usually the versions of what that part reads:
/// `(drafts.version(), writes.version())`. A key missing an input shows stale values, so keep it beside the function it
/// guards, which should take only those inputs.
#[derive(Debug)]
pub struct Slice<K> {
    key: Option<K>,
}

impl<K> Default for Slice<K> {
    fn default() -> Self {
        Slice { key: None }
    }
}

impl<K: PartialEq> Slice<K> {
    /// Runs `apply` on the first call and whenever `key` differs from the last one. True when it ran.
    pub fn update(&mut self, key: K, apply: impl FnOnce()) -> bool {
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        self.key = Some(key);
        apply();
        true
    }
}

/// A [`Slice`] that makes a value before showing it, and so can check itself. In a test build, whenever the key says nothing
/// changed it makes the value anyway and panics if it differs from what is showing: a key missing an input fails there
/// rather than showing stale values in a release, where the value is only made when the key changes.
pub struct CheckedSlice<K, V> {
    key: Option<K>,
    #[cfg(debug_assertions)]
    shown: Option<V>,
    #[cfg(not(debug_assertions))]
    shown: std::marker::PhantomData<V>,
}

impl<K, V> Default for CheckedSlice<K, V> {
    fn default() -> Self {
        CheckedSlice {
            key: None,
            #[cfg(debug_assertions)]
            shown: None,
            #[cfg(not(debug_assertions))]
            shown: std::marker::PhantomData,
        }
    }
}

impl<K: PartialEq, V: PartialEq + Clone> CheckedSlice<K, V> {
    /// Makes the value and shows it, on the first call and whenever `key` differs from the last one. `name` says which slice
    /// a failed check is.
    pub fn update(&mut self, name: &str, key: K, make: impl FnOnce() -> V, show: impl FnOnce(V)) {
        if self.key.as_ref() == Some(&key) {
            #[cfg(debug_assertions)]
            assert!(
                self.shown.as_ref() == Some(&make()),
                "the {name} slice's key is missing an input: it says nothing changed, and its value did"
            );
            #[cfg(not(debug_assertions))]
            let _ = (name, make);
            return;
        }
        self.key = Some(key);
        let value = make();
        #[cfg(debug_assertions)]
        {
            self.shown = Some(value.clone());
        }
        show(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checked_slice_shows_when_its_key_changes() {
        let mut slice = CheckedSlice::default();
        let mut shown = Vec::new();
        slice.update("test", 1, || "a", |value| shown.push(value));
        slice.update("test", 1, || "a", |value| shown.push(value));
        slice.update("test", 2, || "b", |value| shown.push(value));
        assert_eq!(shown, ["a", "b"]);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "key is missing an input")]
    fn a_checked_slice_catches_a_key_that_misses_an_input() {
        let mut slice = CheckedSlice::default();
        slice.update("test", 1, || "a", |_| {});
        slice.update("test", 1, || "b", |_| {});
    }

    #[test]
    fn a_write_moves_the_version_and_a_read_does_not() {
        let mut value = Tracked::new(1);
        let start = value.version();
        assert_eq!(*value, 1);
        assert_eq!(value.version(), start);
        *value = 2;
        assert_ne!(value.version(), start);
    }

    #[test]
    fn setting_an_equal_value_leaves_the_version() {
        let mut value = Tracked::new(1);
        let start = value.version();
        value.set(1);
        assert_eq!(value.version(), start);
        value.set(2);
        assert_ne!(value.version(), start);
        assert_eq!(*value, 2);
    }

    #[test]
    fn a_slice_runs_first_and_then_only_when_its_key_changes() {
        let mut slice = Slice::default();
        let mut runs = 0;
        assert!(slice.update(1, || runs += 1));
        assert!(!slice.update(1, || runs += 1));
        assert!(slice.update(2, || runs += 1));
        assert_eq!(runs, 2);
    }
}
