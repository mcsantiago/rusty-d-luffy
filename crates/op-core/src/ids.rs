//! Stable identifiers.
//!
//! Everything the rules touch is addressed by a dense integer id so that
//! iteration order is structural rather than hash-dependent (see the
//! determinism requirements in `lib.rs`).

use serde::{Deserialize, Serialize};

/// Which of the two players. `PlayerId(0)` is the player who went first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u8);

impl PlayerId {
    pub const P0: PlayerId = PlayerId(0);
    pub const P1: PlayerId = PlayerId(1);

    pub fn opponent(self) -> PlayerId {
        PlayerId(1 - self.0)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A physical card in a game, unique for the whole game. Indexes directly into
/// [`crate::state::GameState::cards`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CardInstanceId(pub u32);

impl CardInstanceId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Index into the [`crate::card::CardDb`]. Printed-card identity, i.e. all four
/// copies of `ST01-003` share one `CardDefId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CardDefId(pub u32);

impl CardDefId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}
