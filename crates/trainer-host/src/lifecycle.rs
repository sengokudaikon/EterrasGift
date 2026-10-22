//! Residency is independent of a pipe acknowledgement. Only native evidence can
//! admit a fresh load; uncertainty permanently fences this process generation.
use std::time::{Duration, Instant};

pub const RESTART_REQUIRED: &str = "The helper could not leave the game; restart Last Epoch before playing online.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Residency {
    Loaded,
    Leaving { deadline: Instant },
    Gone,
    Stuck,
}
impl Residency {
    pub(crate) fn leaving(&mut self, now: Instant, timeout: Duration) {
        if *self == Self::Loaded {
            *self = Self::Leaving { deadline: now + timeout };
        }
    }
    pub(crate) fn observe(&mut self, now: Instant, absent: bool, pristine: bool) {
        if *self == Self::Stuck {
            return;
        }
        if absent && pristine {
            *self = Self::Gone;
        } else if matches!(*self, Self::Leaving { deadline } if now >= deadline) {
            *self = Self::Stuck;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absence_and_pristine_code_are_both_required_and_deadline_never_extends() {
        let now = Instant::now();
        let mut state = Residency::Loaded;
        state.leaving(now, Duration::from_secs(2));
        state.leaving(now + Duration::from_secs(1), Duration::from_secs(2));
        state.observe(now, true, false);
        assert!(matches!(state, Residency::Leaving { .. }));
        state.observe(now, false, true);
        assert!(matches!(state, Residency::Leaving { .. }));
        state.observe(now + Duration::from_secs(2), true, false);
        assert_eq!(state, Residency::Stuck);
        state.observe(now + Duration::from_secs(3), true, true);
        assert_eq!(state, Residency::Stuck);
    }

    #[test]
    fn self_unload_can_be_proven_without_an_app_request() {
        let mut state = Residency::Loaded;
        state.observe(Instant::now(), true, true);
        assert_eq!(state, Residency::Gone);
    }
}
