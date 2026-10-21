/// Eterra's tree in its four painted keyframes, in order. Each surface maps its own real state onto them: Home follows
/// the game (none, found, a character loading, in game) (`spec/shell-and-settings.md` §8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum BloomPhase {
    #[default]
    Wilted,
    Budding,
    Opening,
    Bloom,
}
