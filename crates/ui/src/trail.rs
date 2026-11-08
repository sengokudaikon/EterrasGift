//! The fault trail: each page's store adds the intents it runs to `eg_core::fault::TRAIL`, by name, so a fault report says what
//! the window was doing.

use eg_core::fault::TRAIL;
use spiderland::{Feature, Seen, Store};

/// Adds each intent `store` runs to the trail as `page`'s, just before it is reduced.
pub(crate) fn follow<F: Feature>(store: &Store<F>, page: &'static str)
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
    use spiderland::{Effects, Never, TestStore};

    struct Page;

    #[derive(strum::IntoStaticStr)]
    enum Intent {
        Opened,
    }

    impl Feature for Page {
        type State = ();
        type Intent = Intent;
        type Effect = Never;
        type Output = Never;

        fn reduce(_: &mut (), _: Intent, _: &mut Effects<Never>) {}
    }

    #[test]
    fn a_page_s_intents_reach_the_trail_by_name() {
        let page = TestStore::<Page>::new(());
        follow(page.store(), "a test page");
        page.send(Intent::Opened);
        assert!(TRAIL.lines().iter().any(|line| line.ends_with(" a test page Opened")), "{:?}", TRAIL.lines());
    }
}
