//! The fault trail: each page's store adds the intents it runs to `eg_core::fault::TRAIL`, by name, so a fault report says what
//! the window was doing.

use eg_core::fault::TRAIL;
use spiderland::{Feature, Seen, Wiring};

/// Adds each intent the store runs to the trail as `page`'s, just before it is reduced. On the store's wiring, so the trail
/// has its start too: the intents its first effects answer with at once.
pub(crate) fn follow<F: Feature>(store: &Wiring<F>, page: &'static str)
where
    for<'a> &'a F::Intent: Into<&'static str>,
{
    store.observe(move |seen| {
        if let Seen::Intent(intent) = seen {
            TRAIL.record(page, intent.into());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiderland::{Effects, Host, Intents, Never, Store};

    struct Page;

    #[derive(strum::IntoStaticStr)]
    enum Intent {
        Looked,
        Opened,
    }

    /// Looks as it starts, and the look answers at once, as a host's `Observe` does.
    struct Look;

    impl Feature for Page {
        type State = ();
        type Intent = Intent;
        type Effect = Look;
        type Output = Never;
        type Subscription = Never;

        fn start(_: &mut (), effects: &mut Effects<Look>) {
            effects.push(Look);
        }

        fn reduce(_: &mut (), _: Intent, _: &mut Effects<Look>) {}
    }

    struct Answers;

    impl Host<Page> for Answers {
        fn perform(&mut self, Look: Look, intents: &Intents<Intent>) {
            intents.send(Intent::Looked);
        }
    }

    #[test]
    fn a_page_s_intents_reach_the_trail_by_name_from_its_start() {
        let wiring = Store::<Page>::build((), Answers);
        follow(&wiring, "a test page");
        let page = wiring.start();
        page.send(Intent::Opened);
        let lines = TRAIL.lines();
        assert!(lines.iter().any(|line| line.ends_with(" a test page Looked")), "the start's answer: {lines:?}");
        assert!(lines.iter().any(|line| line.ends_with(" a test page Opened")), "{lines:?}");
    }
}
