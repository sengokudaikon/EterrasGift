//! The navigation in the window: the `navigation` feature's impure half. It shows the feature's state in the `Nav` global, and
//! tells the pages which one shows.

mod navigation;

pub use navigation::{Effect, Intent, NavKey, Navigation, Node, Outcome, Page, Shell, State};

use crate::{MainWindow, Nav, NavOutcome, NavRow, Tokens};
use slint::platform::Key;
use slint::{ComponentHandle, Image, Weak};
use spiderland::{Host, Intents, Slice, Store, Version, callbacks};
use spiderland_slint::Rows;
use std::cell::RefCell;
use std::rc::Rc;

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

type PageEntered = Box<dyn Fn(Page)>;

struct Window {
    window: Weak<MainWindow>,
    rows: Rows<NavRow>,
    footer: Rows<NavRow>,
    /// The rail's flyout of trainer pages.
    flyout: Rows<NavRow>,
    shown: Slice<Version>,
    focused: Slice<i32>,
    entrance: Slice<i32>,
    entered: Rc<RefCell<Vec<PageEntered>>>,
}

/// Connects a `Navigation` to the window's `Nav` global. The handle lets pages follow which one shows.
pub fn bind(window: &MainWindow, navigation: Navigation) -> ShellHandle {
    let entered = Rc::default();
    let host = Window {
        window: window.as_weak(),
        rows: Rows::default(),
        footer: Rows::default(),
        flyout: Rows::default(),
        shown: Slice::default(),
        focused: Slice::default(),
        entrance: Slice::default(),
        entered: Rc::clone(&entered),
    };
    let nav = window.global::<Nav>();
    nav.set_rows(host.rows.model());
    nav.set_footer(host.footer.model());
    nav.set_flyout_pages(host.flyout.model());
    let store = Store::<Shell>::new(State::new(navigation), host);

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
    ShellHandle { store, entered }
}

fn answer(store: &Store<Shell>, intent: Intent) -> NavOutcome {
    match store.ask(intent, State::answer) {
        Some(Outcome::Handled { .. }) => NavOutcome::Handled,
        Some(Outcome::OpenFlyout) => NavOutcome::OpenFlyout,
        Some(Outcome::Ignored) | None => NavOutcome::Ignored,
    }
}

/// The shell as pages see it: which page shows, and word when another one does.
#[derive(Clone)]
pub struct ShellHandle {
    store: Store<Shell>,
    entered: Rc<RefCell<Vec<PageEntered>>>,
}

impl ShellHandle {
    pub fn current(&self) -> Page {
        self.store.try_read(|state| state.navigation().current()).unwrap_or(Page::Home)
    }

    pub fn navigate(&self, page: Page) {
        self.store.send(Intent::Navigate(page));
    }

    /// Calls `entered` each time another page comes to show (not for the page showing now).
    pub fn on_page_entered(&self, entered: impl Fn(Page) + 'static) {
        self.entered.borrow_mut().push(Box::new(entered));
    }

    /// Tells a page whether it shows: now, if it does, and from then on each time another page comes to show.
    pub fn on_shown(&self, page: Page, shown: impl Fn(bool) + 'static) {
        if self.current() == page {
            shown(true);
        }
        self.on_page_entered(move |entered| shown(entered == page));
    }
}

impl Host<Shell> for Window {
    fn render(&mut self, state: &State, _: &Intents<Intent>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
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
        let (row, serial) = state.focus();
        if serial > 0 {
            self.focused.update(serial, || {
                nav.set_focus_row(row as i32);
                nav.set_focus_serial(serial);
            });
        }
        self.entrance.update(state.page_serial(), || nav.set_page_serial(state.page_serial()));
    }

    fn perform(&mut self, Effect::Entered(page): Effect, _: &Intents<Intent>) {
        for entered in self.entered.borrow().iter() {
            entered(page);
        }
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
