/// Eterra's tree in its four painted keyframes, in order. Each surface maps its own real state onto them: Home follows
/// the game (none, found, a character loading, in game). Home and the trainer's pages both show it, so it lives in the
/// kernel they share.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum BloomPhase {
    #[default]
    Wilted,
    Budding,
    Opening,
    Bloom,
}
