//! Game areas (comprehensive rules 3).

use serde::{Deserialize, Serialize};

/// The nine areas a card can occupy (3-1-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Zone {
    Deck,
    DonDeck,
    Hand,
    Trash,
    Leader,
    Character,
    Stage,
    Cost,
    Life,
    /// Not an area. A card whose `[Trigger]` is resolving belongs to no area
    /// while that Trigger is being activated (10-1-5-3).
    Limbo,
}

impl Zone {
    /// Open areas have public contents; secret areas do not (3-1-5).
    ///
    /// The DON!! deck is an open area despite being face-down (3-3-2), and the
    /// Life area is secret even though its card *count* is public (3-1-4).
    pub fn is_open(self) -> bool {
        match self {
            Zone::Trash
            | Zone::Leader
            | Zone::Character
            | Zone::Stage
            | Zone::Cost
            | Zone::DonDeck => true,
            Zone::Deck | Zone::Hand | Zone::Life | Zone::Limbo => false,
        }
    }

    /// The four areas collectively called "the field" (3-1-2). Cards here can
    /// be rested, given DON!!, and targeted by most effects.
    pub fn is_field(self) -> bool {
        matches!(
            self,
            Zone::Leader | Zone::Character | Zone::Stage | Zone::Cost
        )
    }

    /// Whether card order within the area is meaningful and must be preserved.
    pub fn is_ordered(self) -> bool {
        matches!(self, Zone::Deck | Zone::DonDeck | Zone::Life | Zone::Trash)
    }
}
