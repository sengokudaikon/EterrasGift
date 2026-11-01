//! Navigation: which pages exist, which one shows, and how the pane's rows answer clicks and keys. No toolkit in it; `super`
//! shows it in the `Nav` global.

use spiderland::{Effects, Feature, Tracked};
use spiderland_phrases::phrases;

phrases! {
    Home: "Home"
    Mods: "Mods"
    Character: "Character"
    Stash: "Stash"
    Combat: "Combat"
    Forge: "Forge"
    Loot: "Loot"
    Monolith: "Monolith"
    Factions: "Factions"
    World: "World"
    Settings: "Settings"
    Logs: "Logs"
    Trainer: "Trainer"
}

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
            Page::Home => Home.get(),
            Page::Mods => Mods.get(),
            Page::Character => Character.get(),
            Page::Stash => Stash.get(),
            Page::Combat => Combat.get(),
            Page::Forge => Forge.get(),
            Page::Loot => Loot.get(),
            Page::Monolith => Monolith.get(),
            Page::Factions => Factions.get(),
            Page::World => World.get(),
            Page::Logs => Logs.get(),
            Page::Settings => Settings.get(),
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

impl Node {
    pub fn id(self) -> &'static str {
        match self {
            Node::Page(page) => page.id(),
            Node::Trainer => "trainer",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Node::Page(page) => page.title(),
            Node::Trainer => Trainer.get(),
        }
    }
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

    pub fn children_shown(&self) -> bool {
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

/// Navigation as a feature: the model, what the view has been asked to do with it, and word when another page shows.
pub struct Shell;

pub struct State {
    navigation: Tracked<Navigation>,
    /// What the last click or key did, for the view that asked.
    answer: Outcome,
    /// The row to move keyboard focus to, and how many times that has been asked, so the same row can be asked for again.
    focus: (usize, i32),
    /// Counts pages that came to show after the first, for the view's entrance (the first page never fades in).
    page_serial: i32,
    /// The page last shown, to tell when another one comes to show.
    shown: Page,
}

impl State {
    pub fn new(navigation: Navigation) -> State {
        State {
            shown: navigation.current(),
            navigation: Tracked::new(navigation),
            answer: Outcome::Ignored,
            focus: (0, 0),
            page_serial: 0,
        }
    }

    pub fn navigation(&self) -> &Tracked<Navigation> {
        &self.navigation
    }

    pub fn answer(&self) -> Outcome {
        self.answer
    }

    pub fn focus(&self) -> (usize, i32) {
        self.focus
    }

    pub fn page_serial(&self) -> i32 {
        self.page_serial
    }
}

pub enum Intent {
    Click(usize),
    Key(usize, NavKey),
    TogglePane,
    Navigate(Page),
    Wide(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Another page came to show.
    Entered(Page),
}

impl Feature for Shell {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect>) {
        let handled = Outcome::Handled { focus: None };
        state.answer = match intent {
            Intent::Click(index) => state.navigation.click(index),
            Intent::Key(index, key) => state.navigation.key(index, key),
            Intent::TogglePane => {
                state.navigation.toggle_pane();
                handled
            }
            Intent::Navigate(page) => {
                state.navigation.navigate(page);
                handled
            }
            Intent::Wide(wide) => {
                state.navigation.set_wide(wide);
                handled
            }
        };
        if let Outcome::Handled { focus: Some(row) } = state.answer {
            state.focus = (row, state.focus.1 + 1);
        }
        let current = state.navigation.current();
        if state.shown != current {
            state.shown = current;
            state.page_serial += 1;
            effects.push(Effect::Entered(current));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiderland::step;

    #[test]
    fn a_page_coming_to_show_says_so_once_and_counts() {
        let mut state = State::new(navigation());
        assert_eq!(step::<Shell>(&mut state, Intent::Navigate(Page::Logs)), [Effect::Entered(Page::Logs)]);
        assert_eq!(state.page_serial(), 1);
        assert!(step::<Shell>(&mut state, Intent::Navigate(Page::Logs)).is_empty(), "it shows already");
        assert_eq!(state.page_serial(), 1);
    }

    #[test]
    fn the_view_hears_what_its_click_or_key_did_and_where_focus_goes() {
        let mut state = State::new(navigation());
        step::<Shell>(&mut state, Intent::Wide(false));
        let trainer = state.navigation().rows().iter().position(|&node| node == Node::Trainer).unwrap();
        step::<Shell>(&mut state, Intent::Click(trainer));
        assert_eq!(state.answer(), Outcome::OpenFlyout);
        step::<Shell>(&mut state, Intent::Click(999));
        assert_eq!(state.answer(), Outcome::Ignored);
        step::<Shell>(&mut state, Intent::Key(0, NavKey::Down));
        let (row, first) = state.focus();
        assert_eq!(row, 1);
        assert_eq!(state.answer(), Outcome::Handled { focus: Some(1) });
        step::<Shell>(&mut state, Intent::Key(0, NavKey::Down));
        assert_eq!(state.focus(), (1, first + 1), "the same row again still moves focus");
    }

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
