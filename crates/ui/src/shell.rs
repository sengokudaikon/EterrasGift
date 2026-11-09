//! The navigation in the window: the `navigation` feature's impure half. It shows the feature's state in the `Nav` global, and
//! tells the pages which one shows.

mod navigation;

pub use navigation::{Entered, Intent, NavKey, Navigation, Node, Outcome, Page, Shell, State};

use crate::{MainWindow, Nav, NavOutcome, NavRow, Tokens};
use slint::platform::Key;
use slint::{ComponentHandle, Image};
use spiderland::{Intents, Never, Slice, Store, Version, callbacks};
use spiderland_slint::{Rows, WindowHost, Windowed};

fn key_of(text: &str) -> Option<NavKey> {
    let key = text.chars().next()?;
    let is = |k: Key| key == char::from(k);
    Some(if is(Key::UpArrow) {
        NavKey::Up
    } else if is(Key::DownArrow) {
        NavKey::Down
    } else if is(Key::Home) || is(Key::PageUp) {
        NavKey::First
    } else if is(Key::End) || is(Key::PageDown) {
        NavKey::Last
    } else if is(Key::LeftArrow) {
        NavKey::Left
    } else if is(Key::RightArrow) {
        NavKey::Right
    } else if is(Key::Return) || key == ' ' {
        NavKey::Activate
    } else {
        return None;
    })
}

struct Window {
    rows: Rows<NavRow>,
    footer: Rows<NavRow>,
    /// The rail's flyout of trainer pages.
    flyout: Rows<NavRow>,
    shown: Slice<Version>,
    focused: Slice<i32>,
    entrance: Slice<i32>,
}

/// Connects a `Navigation` to the window's `Nav` global. The handle lets pages follow which one shows.
pub fn bind(window: &MainWindow, navigation: Navigation) -> ShellHandle {
    let host = Window {
        rows: Rows::default(),
        footer: Rows::default(),
        flyout: Rows::default(),
        shown: Slice::default(),
        focused: Slice::default(),
        entrance: Slice::default(),
    };
    let nav = window.global::<Nav>();
    nav.set_rows(host.rows.model());
    nav.set_footer(host.footer.model());
    nav.set_flyout_pages(host.flyout.model());
    let wiring = Store::<Shell>::build(State::new(navigation), Windowed::new(window, host));
    crate::trail::follow(&wiring, "shell");
    let store = wiring.start();

    callbacks!(nav, store, {
        on_toggle_pane: || Intent::TogglePane,
        on_wide_changed: |wide| Intent::Wide(wide),
    });
    // The view asks what a click or key did, and hears it from the reducer.
    nav.on_row_clicked({
        let store = store.clone();
        move |index| answer(&store, Intent::Click(index as usize))
    });
    nav.on_row_key({
        let store = store.clone();
        move |index, text, modified| match key_of(&text) {
            Some(key) if !modified => answer(&store, Intent::Key(index as usize, key)),
            _ => NavOutcome::Ignored,
        }
    });
    nav.on_navigate({
        let store = store.clone();
        move |id| {
            if let Some(page) = Page::from_id(&id) {
                store.send(Intent::Navigate(page));
            }
        }
    });
    ShellHandle { store }
}

fn answer(store: &Store<Shell>, intent: Intent) -> NavOutcome {
    match store.ask(intent, State::answer) {
        Some(Outcome::Handled { .. }) => NavOutcome::Handled,
        Some(Outcome::OpenFlyout) => NavOutcome::OpenFlyout,
        Some(Outcome::Ignored) | None => NavOutcome::Ignored,
    }
}

/// The shell as pages see it: which page shows, word when another one does, and where to ask for one.
#[derive(Clone)]
pub struct ShellHandle {
    store: Store<Shell>,
}

impl ShellHandle {
    pub fn current(&self) -> Page {
        self.store.try_read(|state| state.navigation().current()).unwrap_or(Page::Home)
    }

    /// The shell's sender, for a page's store to forward its request to show as [`Intent::Navigate`].
    pub fn intents(&self) -> Intents<Intent> {
        self.store.intents()
    }

    /// Turns each page that comes to show (not the page showing now) into `to`'s intent, or none.
    pub fn forward<I: 'static, R: Into<Option<I>>>(&self, to: &Intents<I>, entered: impl Fn(Page) -> R + 'static) {
        self.store.forward(to, move |&Entered(page)| entered(page));
    }

    /// Tells a page whether it shows each time another page comes to show. Whether it shows at first is [`Self::current`],
    /// for its initial state.
    pub fn forward_shown<I: 'static>(&self, page: Page, to: &Intents<I>, shown: impl Fn(bool) -> I + 'static) {
        self.forward(to, move |entered| shown(entered == page));
    }
}

impl WindowHost<Shell> for Window {
    type Window = MainWindow;

    fn render(&mut self, window: &MainWindow, state: &State, _: &Intents<Intent>) {
        let tokens = window.global::<Tokens>();
        let nav = window.global::<Nav>();
        let navigation = state.navigation();
        let (rows, footer, flyout) = (&self.rows, &self.footer, &self.flyout);
        self.shown.update(navigation.version(), || {
            let row = |node: Node| nav_row(navigation, &tokens, node);
            // The trainer's pages come and go only after the Trainer item, the last row, so rows that stay keep their place and
            // keyboard focus.
            rows.sync(navigation.rows().into_iter().map(row));
            footer.sync(navigation.footer().into_iter().map(row));
            flyout.sync(navigation.trainer_pages().iter().map(|&page| row(Node::Page(page))));
            nav.set_current_page(navigation.current().id().into());
            nav.set_current_title(navigation.current().title().into());
            nav.set_pane_open(navigation.pane_open());
            nav.set_tab_row(navigation.tab_row() as i32);
        });
        // A pulse's value goes first, so the view acts on it when its serial moves.
        let focus = state.focus();
        self.focused.update(focus.serial(), || {
            nav.set_focus_row(*focus.value() as i32);
            nav.set_focus_serial(focus.serial());
        });
        let entrance = state.entrance().serial();
        self.entrance.update(entrance, || nav.set_page_serial(entrance));
    }

    fn perform(&mut self, _: Option<&MainWindow>, effect: Never, _: &Intents<Intent>) {
        match effect {}
    }
}

fn nav_row(navigation: &Navigation, tokens: &Tokens<'_>, node: Node) -> NavRow {
    let icon = match node {
        Node::Page(page) => page_icon(tokens, page),
        Node::Trainer => tokens.get_icon_trainer(),
    };
    NavRow {
        id: node.id().into(),
        title: node.title().into(),
        icon,
        child: navigation.is_child(node),
        parent: node == Node::Trainer,
        expanded: node == Node::Trainer && navigation.children_shown(),
        selected: navigation.is_selected(node),
    }
}

fn page_icon(tokens: &Tokens<'_>, page: Page) -> Image {
    match page {
        Page::Home => tokens.get_icon_home(),
        Page::Mods => tokens.get_icon_mods(),
        Page::Character => tokens.get_icon_character(),
        Page::Stash => tokens.get_icon_stash(),
        Page::Combat => tokens.get_icon_combat(),
        Page::Forge => tokens.get_icon_forge(),
        Page::Loot => tokens.get_icon_loot(),
        Page::Monolith => tokens.get_icon_monolith(),
        Page::Factions => tokens.get_icon_factions(),
        Page::World => tokens.get_icon_world(),
        Page::Logs => tokens.get_icon_bug(),
        Page::Settings => tokens.get_icon_settings(),
    }
}
