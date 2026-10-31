//! Navigation: which pages exist, which one shows, and how the pane's rows answer clicks and keys. The model is plain
//! Rust; `bind` connects it to the `Nav` global.

use crate::models::update_rows;
use crate::{MainWindow, Nav, NavOutcome, NavRow, Tokens};
use eg_core::phrase::Phrase;
use slint::platform::Key;
use slint::{ComponentHandle, Image, ModelRc, VecModel, Weak};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const HOME: Phrase = Phrase::new("Home");
const MODS: Phrase = Phrase::new("Mods");
const CHARACTER: Phrase = Phrase::new("Character");
const STASH: Phrase = Phrase::new("Stash");
const COMBAT: Phrase = Phrase::new("Combat");
const FORGE: Phrase = Phrase::new("Forge");
const LOOT: Phrase = Phrase::new("Loot");
const MONOLITH: Phrase = Phrase::new("Monolith");
const FACTIONS: Phrase = Phrase::new("Factions");
const WORLD: Phrase = Phrase::new("World");
const SETTINGS: Phrase = Phrase::new("Settings");
const LOGS: Phrase = Phrase::new("Logs");
const TRAINER: Phrase = Phrase::new("Trainer");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    Mods,
    Character,
    Stash,
    Combat,
    Forge,
    Loot,
    Monolith,
    Factions,
    World,
    Logs,
    Settings,
}

impl Page {
    /// The trainer's pages, one per area, in the pane's order.
    pub const TRAINER: [Page; 8] = [
        Page::Character,
        Page::Stash,
        Page::Combat,
        Page::Forge,
        Page::Loot,
        Page::Monolith,
        Page::Factions,
        Page::World,
    ];

    const ALL: [Page; 12] = [
        Page::Home,
        Page::Mods,
        Page::Character,
        Page::Stash,
        Page::Combat,
        Page::Forge,
        Page::Loot,
        Page::Monolith,
        Page::Factions,
        Page::World,
        Page::Logs,
        Page::Settings,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Mods => "mods",
            Page::Character => "character",
            Page::Stash => "stash",
            Page::Combat => "combat",
            Page::Forge => "forge",
            Page::Loot => "loot",
            Page::Monolith => "monolith",
            Page::Factions => "factions",
            Page::World => "world",
            Page::Logs => "logs",
            Page::Settings => "settings",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Home => HOME.get(),
            Page::Mods => MODS.get(),
            Page::Character => CHARACTER.get(),
            Page::Stash => STASH.get(),
            Page::Combat => COMBAT.get(),
            Page::Forge => FORGE.get(),
            Page::Loot => LOOT.get(),
            Page::Monolith => MONOLITH.get(),
            Page::Factions => FACTIONS.get(),
            Page::World => WORLD.get(),
            Page::Logs => LOGS.get(),
            Page::Settings => SETTINGS.get(),
        }
    }

    pub fn from_id(id: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|page| page.id() == id)
    }

    pub fn is_trainer(self) -> bool {
        Page::TRAINER.contains(&self)
    }
}

/// A row of the pane: a page, or the Trainer item, which holds the trainer's pages but is never a page itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    Page(Page),
    Trainer,
}

/// The keys the rows answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    First,
    Last,
    Left,
    Right,
    Activate,
}

impl NavKey {
    fn from_text(text: &str) -> Option<NavKey> {
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
}

/// What a click or key did, for the view: nothing, something (maybe moving keyboard focus to a row), or open the rail's
/// flyout of trainer pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ignored,
    Handled { focus: Option<usize> },
    OpenFlyout,
}

#[derive(Debug)]
pub struct Navigation {
    has_mods: bool,
    trainer_pages: Vec<Page>,
    current: Page,
    pane_open: bool,
    trainer_open: bool,
    wide: Option<bool>,
}

impl Navigation {
    /// `trainer_pages` is empty when the trainer is off; Mods is listed only when the launcher and mod library exist.
    pub fn new(has_mods: bool, trainer_pages: Vec<Page>) -> Self {
        Navigation {
            has_mods,
            trainer_pages,
            current: Page::Home,
            pane_open: false,
            trainer_open: false,
            wide: None,
        }
    }

    pub fn current(&self) -> Page {
        self.current
    }

    pub fn pane_open(&self) -> bool {
        self.pane_open
    }

    pub fn trainer_pages(&self) -> &[Page] {
        &self.trainer_pages
    }

    fn children_shown(&self) -> bool {
        self.pane_open && self.trainer_open
    }

    /// The pane's rows, above the foot.
    pub fn rows(&self) -> Vec<Node> {
        let mut rows = vec![Node::Page(Page::Home)];
        if self.has_mods {
            rows.push(Node::Page(Page::Mods));
        }
        if !self.trainer_pages.is_empty() {
            rows.push(Node::Trainer);
            if self.children_shown() {
                rows.extend(self.trainer_pages.iter().map(|&page| Node::Page(page)));
            }
        }
        rows
    }

    /// Diagnostics and Settings stay at the foot, available from every page.
    pub fn footer(&self) -> Vec<Node> {
        vec![Node::Page(Page::Logs), Node::Page(Page::Settings)]
    }

    /// Rows and foot as the keyboard sees them: one list.
    fn all(&self) -> Vec<Node> {
        let mut all = self.rows();
        all.extend(self.footer());
        all
    }

    fn index_of(&self, node: Node) -> Option<usize> {
        self.all().iter().position(|&n| n == node)
    }

    pub fn is_child(&self, node: Node) -> bool {
        matches!(node, Node::Page(page) if page.is_trainer()) && self.children_shown()
    }

    /// The current page's row carries the selected mark; while the trainer's pages are hidden, the Trainer item does.
    pub fn is_selected(&self, node: Node) -> bool {
        match node {
            Node::Page(page) => page == self.current,
            Node::Trainer => self.current.is_trainer() && !self.children_shown(),
        }
    }

    /// The row that takes the navigation's single tab stop.
    pub fn tab_row(&self) -> usize {
        self.all().iter().position(|&node| self.is_selected(node)).unwrap_or(0)
    }

    pub fn navigate(&mut self, page: Page) {
        self.current = page;
        // Navigating to a trainer page while the pane is expanded opens the item, so the current row is visible.
        self.reveal_current();
    }

    /// Expanding the pane reveals the current page: a trainer page opens the Trainer item, so its row shows.
    pub fn toggle_pane(&mut self) {
        self.pane_open = !self.pane_open;
        self.reveal_current();
    }

    fn reveal_current(&mut self) {
        if self.pane_open && self.current.is_trainer() {
            self.trainer_open = true;
        }
    }

    /// Crossing the wide threshold sets the pane: expanded when wide, the rail below. The user's toggle holds in
    /// between; the threshold applies again only when it is crossed again.
    pub fn set_wide(&mut self, wide: bool) {
        if self.wide != Some(wide) {
            self.wide = Some(wide);
            self.pane_open = wide;
            self.reveal_current();
        }
    }

    /// A left-click on a row: a page navigates; the Trainer item opens or closes (on the rail, shows its pages).
    pub fn click(&mut self, index: usize) -> Outcome {
        match self.all().get(index) {
            Some(&Node::Page(page)) => {
                self.navigate(page);
                Outcome::Handled { focus: None }
            }
            Some(Node::Trainer) => self.invoke_trainer(),
            None => Outcome::Ignored,
        }
    }

    pub fn key(&mut self, index: usize, key: NavKey) -> Outcome {
        let all = self.all();
        let Some(&node) = all.get(index) else {
            return Outcome::Ignored;
        };
        let target = match key {
            NavKey::Up if index > 0 => Some(all[index - 1]),
            NavKey::Down if index + 1 < all.len() => Some(all[index + 1]),
            NavKey::First => all.first().copied(),
            NavKey::Last => all.last().copied(),
            NavKey::Left if self.is_child(node) => Some(Node::Trainer),
            NavKey::Right if node == Node::Trainer && self.children_shown() => all.get(index + 1).copied(),
            _ => None,
        };
        if let Some(target) = target {
            return self.move_to(target);
        }
        let opens = node == Node::Trainer && (matches!(key, NavKey::Activate | NavKey::Right) || (key == NavKey::Left && self.children_shown()));
        if opens {
            return self.invoke_trainer();
        }
        match key {
            // The list's own keys stop here even at its ends, so they never scroll or move anything else.
            NavKey::Up | NavKey::Down | NavKey::First | NavKey::Last => Outcome::Handled { focus: None },
            _ => Outcome::Ignored,
        }
    }

    /// Arrow keys choose each page they reach, as an arrow in a list does; on the Trainer item they stop without
    /// choosing it.
    fn move_to(&mut self, target: Node) -> Outcome {
        if let Node::Page(page) = target {
            self.navigate(page);
        }
        Outcome::Handled { focus: self.index_of(target) }
    }

    fn invoke_trainer(&mut self) -> Outcome {
        if self.pane_open {
            self.trainer_open = !self.trainer_open;
            Outcome::Handled {
                focus: self.index_of(Node::Trainer),
            }
        } else {
            Outcome::OpenFlyout
        }
    }
}

/// Connects a `Navigation` to the window's `Nav` global and keeps the two in step. The handle lets pages follow
/// which one shows.
pub fn bind(window: &MainWindow, navigation: Navigation) -> ShellHandle {
    let shell = Rc::new(Shell {
        shown: Cell::new(navigation.current()),
        navigation: RefCell::new(navigation),
        rows: Rc::new(VecModel::default()),
        footer: Rc::new(VecModel::default()),
        flyout: Rc::new(VecModel::default()),
        window: window.as_weak(),
        focus_serial: RefCell::new(0),
        page_serial: Cell::new(0),
        entered: RefCell::new(Vec::new()),
    });
    let nav = window.global::<Nav>();
    nav.set_rows(ModelRc::from(shell.rows.clone()));
    nav.set_footer(ModelRc::from(shell.footer.clone()));
    nav.set_flyout_pages(ModelRc::from(shell.flyout.clone()));

    nav.on_row_clicked({
        let shell = shell.clone();
        move |index| shell.apply(|navigation| navigation.click(index as usize))
    });
    nav.on_row_key({
        let shell = shell.clone();
        move |index, text, modified| match NavKey::from_text(&text) {
            Some(key) if !modified => shell.apply(|navigation| navigation.key(index as usize, key)),
            _ => NavOutcome::Ignored,
        }
    });
    nav.on_toggle_pane({
        let shell = shell.clone();
        move || {
            shell.apply(|navigation| {
                navigation.toggle_pane();
                Outcome::Handled { focus: None }
            });
        }
    });
    nav.on_navigate({
        let shell = shell.clone();
        move |id| {
            if let Some(page) = Page::from_id(&id) {
                shell.apply(|navigation| {
                    navigation.navigate(page);
                    Outcome::Handled { focus: None }
                });
            }
        }
    });
    nav.on_wide_changed({
        let shell = shell.clone();
        move |wide| {
            shell.apply(|navigation| {
                navigation.set_wide(wide);
                Outcome::Handled { focus: None }
            });
        }
    });
    shell.sync(None);
    ShellHandle(shell)
}

/// The shell as pages see it: which page shows, and word when another one does.
#[derive(Clone)]
pub struct ShellHandle(Rc<Shell>);

impl ShellHandle {
    pub fn current(&self) -> Page {
        self.0.navigation.borrow().current()
    }

    pub fn navigate(&self, page: Page) {
        self.0.apply(|navigation| {
            navigation.navigate(page);
            Outcome::Handled { focus: None }
        });
    }

    /// Calls `entered` each time another page comes to show (not for the page showing now).
    pub fn on_page_entered(&self, entered: impl Fn(Page) + 'static) {
        self.0.entered.borrow_mut().push(Box::new(entered));
    }
}

type PageEntered = Box<dyn Fn(Page)>;

struct Shell {
    navigation: RefCell<Navigation>,
    rows: Rc<VecModel<NavRow>>,
    footer: Rc<VecModel<NavRow>>,
    /// The rail's flyout of trainer pages.
    flyout: Rc<VecModel<NavRow>>,
    window: Weak<MainWindow>,
    focus_serial: RefCell<i32>,
    /// Counts pages that came to show after the first, for the view's entrance (the first page never fades in).
    page_serial: Cell<i32>,
    /// The page last synced to the view, to tell when another one comes to show.
    shown: Cell<Page>,
    entered: RefCell<Vec<PageEntered>>,
}

impl Shell {
    fn apply(&self, action: impl FnOnce(&mut Navigation) -> Outcome) -> NavOutcome {
        let outcome = action(&mut self.navigation.borrow_mut());
        match outcome {
            Outcome::Ignored => NavOutcome::Ignored,
            Outcome::Handled { focus } => {
                self.sync(focus);
                NavOutcome::Handled
            }
            Outcome::OpenFlyout => {
                self.sync(None);
                NavOutcome::OpenFlyout
            }
        }
    }

    fn sync(&self, focus: Option<usize>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let navigation = self.navigation.borrow();
        let tokens = window.global::<Tokens>();
        let nav = window.global::<Nav>();
        let row = |node: Node| nav_row(&navigation, &tokens, node);

        // The trainer's pages come and go only after the Trainer item, the last row, so rows that stay keep their place and
        // keyboard focus.
        update_rows(&self.rows, navigation.rows().into_iter().map(row).collect());
        update_rows(&self.footer, navigation.footer().into_iter().map(row).collect());
        update_rows(&self.flyout, navigation.trainer_pages().iter().map(|&page| row(Node::Page(page))).collect());
        nav.set_current_page(navigation.current().id().into());
        nav.set_current_title(navigation.current().title().into());
        nav.set_pane_open(navigation.pane_open());
        nav.set_tab_row(navigation.tab_row() as i32);
        if let Some(index) = focus {
            let mut serial = self.focus_serial.borrow_mut();
            *serial += 1;
            nav.set_focus_row(index as i32);
            nav.set_focus_serial(*serial);
        }
        let current = navigation.current();
        drop(navigation);
        if self.shown.replace(current) != current {
            self.page_serial.set(self.page_serial.get() + 1);
            nav.set_page_serial(self.page_serial.get());
            for entered in self.entered.borrow().iter() {
                entered(current);
            }
        }
    }
}

fn nav_row(navigation: &Navigation, tokens: &Tokens<'_>, node: Node) -> NavRow {
    let (id, title, icon) = match node {
        Node::Page(page) => (page.id(), page.title(), page_icon(tokens, page)),
        Node::Trainer => ("trainer", TRAINER.get(), tokens.get_icon_trainer()),
    };
    NavRow {
        id: id.into(),
        title: title.into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn navigation() -> Navigation {
        Navigation::new(true, Page::TRAINER.to_vec())
    }

    fn ids(nodes: &[Node]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| match node {
                Node::Page(page) => page.id().to_owned(),
                Node::Trainer => "trainer".to_owned(),
            })
            .collect()
    }

    #[test]
    fn trainer_pages_show_only_in_the_open_pane() {
        let mut nav = navigation();
        assert_eq!(ids(&nav.rows()), ["home", "mods", "trainer"]);
        nav.toggle_pane();
        assert_eq!(nav.click(2), Outcome::Handled { focus: Some(2) });
        assert_eq!(nav.rows().len(), 3 + Page::TRAINER.len());
        nav.toggle_pane();
        assert_eq!(ids(&nav.rows()), ["home", "mods", "trainer"]);
    }

    #[test]
    fn the_trainer_item_opens_a_flyout_on_the_rail() {
        let mut nav = navigation();
        assert_eq!(nav.click(2), Outcome::OpenFlyout);
        assert_eq!(nav.key(2, NavKey::Activate), Outcome::OpenFlyout);
        assert_eq!(nav.current(), Page::Home);
    }

    #[test]
    fn arrows_navigate_but_stop_on_the_trainer_item() {
        let mut nav = navigation();
        assert_eq!(nav.key(0, NavKey::Down), Outcome::Handled { focus: Some(1) });
        assert_eq!(nav.current(), Page::Mods);
        assert_eq!(nav.key(1, NavKey::Down), Outcome::Handled { focus: Some(2) });
        assert_eq!(nav.current(), Page::Mods, "landing on the Trainer item does not navigate");
        assert_eq!(nav.key(2, NavKey::Down), Outcome::Handled { focus: Some(3) });
        assert_eq!(nav.current(), Page::Logs, "rows and foot are one list");
        assert_eq!(nav.key(3, NavKey::Down), Outcome::Handled { focus: Some(4) });
        assert_eq!(nav.current(), Page::Settings);
        assert_eq!(nav.key(4, NavKey::Down), Outcome::Handled { focus: None });
        assert_eq!(nav.key(4, NavKey::First), Outcome::Handled { focus: Some(0) });
        assert_eq!(nav.current(), Page::Home);
    }

    #[test]
    fn right_and_left_move_between_the_trainer_and_its_pages() {
        let mut nav = navigation();
        nav.toggle_pane();
        assert_eq!(nav.key(2, NavKey::Right), Outcome::Handled { focus: Some(2) }, "Right opens it");
        assert_eq!(nav.key(2, NavKey::Right), Outcome::Handled { focus: Some(3) }, "then moves to its first page");
        assert_eq!(nav.current(), Page::Character);
        assert_eq!(nav.key(3, NavKey::Left), Outcome::Handled { focus: Some(2) }, "Left on a page goes up");
        assert_eq!(nav.key(2, NavKey::Left), Outcome::Handled { focus: Some(2) }, "Left on the open item closes it");
        assert_eq!(nav.rows().len(), 3);
        assert!(nav.is_selected(Node::Trainer), "the closed item carries the current trainer page's mark");
    }

    #[test]
    fn navigating_to_a_trainer_page_in_the_pane_opens_the_item() {
        let mut nav = navigation();
        nav.toggle_pane();
        nav.navigate(Page::Forge);
        assert!(nav.rows().contains(&Node::Page(Page::Forge)));
        assert!(nav.is_selected(Node::Page(Page::Forge)));
        assert!(!nav.is_selected(Node::Trainer));
    }

    #[test]
    fn expanding_the_pane_on_a_trainer_page_opens_the_item() {
        let mut nav = navigation();
        nav.navigate(Page::Forge);
        assert!(nav.is_selected(Node::Trainer), "on the rail the Trainer item carries the mark");
        nav.toggle_pane();
        assert!(nav.rows().contains(&Node::Page(Page::Forge)), "the expanded pane shows the current page's row");
        assert!(nav.is_selected(Node::Page(Page::Forge)));
        assert!(!nav.is_selected(Node::Trainer));
        assert_eq!(nav.tab_row(), nav.index_of(Node::Page(Page::Forge)).unwrap());
    }

    #[test]
    fn expanding_the_pane_on_another_page_leaves_the_item_closed() {
        let mut nav = navigation();
        nav.toggle_pane();
        assert_eq!(ids(&nav.rows()), ["home", "mods", "trainer"]);
    }

    #[test]
    fn the_wide_threshold_applies_only_when_crossed() {
        let mut nav = navigation();
        nav.set_wide(false);
        assert!(!nav.pane_open());
        nav.toggle_pane();
        nav.set_wide(false);
        assert!(nav.pane_open(), "the user's toggle holds until the threshold is crossed");
        nav.set_wide(true);
        nav.set_wide(false);
        assert!(!nav.pane_open());
    }

    #[test]
    fn a_run_without_the_trainer_has_no_trainer_item() {
        let nav = Navigation::new(false, Vec::new());
        assert_eq!(ids(&nav.rows()), ["home"]);
        assert_eq!(ids(&nav.footer()), ["logs", "settings"]);
    }
}
