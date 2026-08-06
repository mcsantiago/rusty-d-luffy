//! Per-player redacted views.
//!
//! [`GameState`] is omniscient and must never reach a client. Everything a
//! client or an imperfect-information agent sees goes through [`PlayerView`],
//! which replaces the contents of secret areas (3-1-5) with counts.
//!
//! The server's send path takes a `PlayerView`, so leaking hidden information
//! is a type error rather than a review comment.

use serde::{Deserialize, Serialize};

use crate::action::Pending;
use crate::card::CardDb;
use crate::ids::{CardInstanceId, PlayerId};
use crate::state::{BattleState, GameOver, GameState, Phase};

/// A card as the viewer can see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisibleCard {
    pub id: CardInstanceId,
    /// Card number, or `None` when the viewer may not know what it is.
    pub number: Option<String>,
    pub rested: bool,
    pub attached_don: usize,
    /// Power as currently derived. `None` for cards with no power.
    pub power: Option<i32>,
}

/// One player's areas as seen by the viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSide {
    pub player: PlayerId,
    pub leader: Option<VisibleCard>,
    pub characters: Vec<VisibleCard>,
    pub stage: Option<VisibleCard>,
    /// The cost area, card by card. It is an open area (3-1-5), so both
    /// players may see it in full — and effects that target a specific DON!!
    /// (ST02-008) need the ids to be addressable.
    pub don: Vec<VisibleCard>,
    /// Convenience totals over [`PlayerSide::don`].
    pub don_active: usize,
    pub don_rested: usize,
    pub don_deck: usize,
    /// The viewer's own hand. **Always empty for the opponent** — see
    /// [`PlayerSide::hand_count`].
    ///
    /// The opponent's hand cannot be represented as redacted `VisibleCard`s,
    /// because a `CardInstanceId` is not an opaque token: ids are assigned in
    /// decklist order at setup, so shipping one for a hidden card leaks the
    /// card to anyone who knows the decklist. Only the count crosses.
    pub hand: Vec<VisibleCard>,
    pub hand_count: usize,
    pub deck_count: usize,
    /// Life card count. Contents are secret to both players.
    pub life_count: usize,
    /// The trash is an open area (3-5-2).
    pub trash: Vec<VisibleCard>,
}

/// The game from one player's seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerView {
    pub viewer: PlayerId,
    pub turn: u32,
    pub turn_player: PlayerId,
    pub phase: Phase,
    pub battle: Option<BattleState>,
    pub game_over: Option<GameOver>,
    /// Present only when the viewer is the one being asked.
    pub pending: Option<Pending>,
    pub you: PlayerSide,
    pub opponent: PlayerSide,
}

impl PlayerView {
    /// Projects the omniscient state down to what `viewer` may see.
    pub fn project(
        state: &GameState,
        db: &CardDb,
        derived: &crate::derive::Derived,
        viewer: PlayerId,
    ) -> PlayerView {
        PlayerView {
            viewer,
            turn: state.turn,
            turn_player: state.turn_player,
            phase: state.phase,
            battle: state.battle.clone(),
            game_over: state.game_over,
            pending: state
                .pending
                .clone()
                .filter(|p| p.player() == viewer),
            you: side(state, db, derived, viewer, true),
            opponent: side(state, db, derived, viewer.opponent(), false),
        }
    }
}

fn side(
    state: &GameState,
    db: &CardDb,
    derived: &crate::derive::Derived,
    player: PlayerId,
    is_viewer: bool,
) -> PlayerSide {
    let ps = state.player(player);
    let visible = |id: CardInstanceId| visible_card(state, db, derived, id, true);

    PlayerSide {
        player,
        leader: ps.leader.map(visible),
        characters: ps.characters.iter().copied().map(visible).collect(),
        stage: ps.stage.map(visible),
        don: ps.cost_area.iter().copied().map(visible).collect(),
        don_active: ps
            .cost_area
            .iter()
            .filter(|&&d| state.card(d).is_active())
            .count(),
        don_rested: ps
            .cost_area
            .iter()
            .filter(|&&d| !state.card(d).is_active())
            .count(),
        don_deck: ps.don_deck.len(),
        // 3-4-2: the hand is a secret area. The viewer sees their own, and of
        // the opponent's learns only the size (3-1-4).
        hand: if is_viewer {
            ps.hand.iter().copied().map(visible).collect()
        } else {
            Vec::new()
        },
        hand_count: ps.hand.len(),
        // 3-2-2: neither player may see the deck.
        deck_count: ps.deck.len(),
        // 3-1-4: the Life count is public, the contents are not.
        life_count: ps.life.len(),
        trash: ps.trash.iter().copied().map(visible).collect(),
    }
}

fn visible_card(
    state: &GameState,
    db: &CardDb,
    derived: &crate::derive::Derived,
    id: CardInstanceId,
    revealed: bool,
) -> VisibleCard {
    let card = state.card(id);
    VisibleCard {
        id,
        number: revealed.then(|| db.get(card.def).number.clone()),
        rested: card.rested,
        attached_don: card.attached_don.len(),
        power: revealed
            .then(|| db.get(card.def).power.map(|_| derived.power(id)))
            .flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every card identity in a view must be one the viewer is entitled to.
    /// This is the property the whole hidden-information design rests on.
    pub fn assert_no_leaks(view: &PlayerView, state: &GameState, db: &CardDb) {
        for card in &view.opponent.hand {
            assert!(
                card.number.is_none(),
                "opponent hand leaked card identity: {card:?}"
            );
        }
        // The Life area is not projected as cards at all, only as a count.
        let opp = view.viewer.opponent();
        assert_eq!(view.opponent.life_count, state.player(opp).life.len());
        assert_eq!(view.opponent.deck_count, state.player(opp).deck.len());
        let _ = db;
    }
}
