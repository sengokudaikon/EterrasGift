//! Pure watchdog policy. Menus are departure; a zone change within InGame is not.
use eg_game_access::il2cpp::discovery::presence::PlayPresence;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(250);
const UNKNOWN_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct Watchdog {
    next: Option<Instant>,
    unknown_since: Option<Instant>,
    presence: PlayPresence,
}
impl Watchdog {
    pub(crate) const fn new() -> Self {
        Self {
            next: None,
            unknown_since: None,
            presence: PlayPresence::Unknown,
        }
    }
    pub(crate) fn due(&mut self, now: Instant) -> bool {
        if self.next.is_some_and(|next| now < next) {
            return false;
        }
        self.next = Some(now + POLL);
        true
    }
    pub(crate) fn observe(&mut self, now: Instant, presence: PlayPresence) -> bool {
        self.presence = presence;
        match presence {
            PlayPresence::Online | PlayPresence::Menu => true,
            PlayPresence::InGame => {
                self.unknown_since = None;
                false
            }
            PlayPresence::Unknown => now.duration_since(*self.unknown_since.get_or_insert(now)) >= UNKNOWN_TIMEOUT,
        }
    }
    pub(crate) fn admits_work(&self) -> bool {
        self.presence == PlayPresence::InGame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menus_and_online_stop_immediately_but_game_travel_never_uses_the_absence_timer() {
        let now = Instant::now();
        let mut watch = Watchdog::new();
        for seconds in [0, 1, 5, 60] {
            assert!(!watch.observe(now + Duration::from_secs(seconds), PlayPresence::InGame));
        }
        assert!(watch.admits_work());
        assert!(watch.observe(now + Duration::from_secs(61), PlayPresence::Menu));
        assert!(!watch.admits_work());
        assert!(watch.observe(now + Duration::from_secs(62), PlayPresence::Online));
    }
    #[test]
    fn unknown_state_is_bounded_and_reentry_resets_its_timer() {
        let now = Instant::now();
        let mut watch = Watchdog::new();
        assert!(watch.due(now));
        assert!(!watch.due(now + Duration::from_millis(249)));
        assert!(watch.due(now + POLL));
        assert!(!watch.observe(now, PlayPresence::Unknown));
        assert!(!watch.admits_work());
        assert!(!watch.observe(now + Duration::from_secs(2), PlayPresence::InGame));
        assert!(!watch.observe(now + Duration::from_secs(4), PlayPresence::Unknown));
        assert!(!watch.observe(now + Duration::from_secs(6), PlayPresence::Unknown));
        assert!(watch.observe(now + Duration::from_secs(7), PlayPresence::Unknown));
    }
}
