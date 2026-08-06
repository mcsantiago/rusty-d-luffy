//! Agents for the One Piece Card Game engine.
//!
//! Every agent picks from [`op_core::legal_actions`], so an agent can never
//! produce an illegal action, and the server's validator and the RL action mask
//! stay in agreement with what agents can do.
//!
//! Agents receive `&Game` for convenience. Any agent that must not see hidden
//! information is responsible for destroying it first — see
//! [`determinize::determinize`], which [`ismcts::IsmctsAgent`] applies at the
//! root of every search iteration.

pub mod determinize;
pub mod heuristic;
pub mod ismcts;

use op_core::{Action, Game, PlayerId};

/// Something that can choose an action.
pub trait Agent {
    /// Returns an action legal in `game` for `player`.
    fn choose(&mut self, game: &Game, player: PlayerId) -> Action;
}

pub use heuristic::{evaluate, HeuristicAgent};
pub use ismcts::{IsmctsAgent, IsmctsConfig};

/// Plays a full game between two agents and reports the winner.
///
/// Both agents answer whatever decision is pending, including the opponent's
/// blocks and counters, so this drives the whole game rather than only turns.
pub fn play_out(
    game: &mut Game,
    p0: &mut dyn Agent,
    p1: &mut dyn Agent,
    max_actions: usize,
) -> Option<op_core::GameOver> {
    for _ in 0..max_actions {
        if game.is_over() {
            break;
        }
        let Some(pending) = game.pending() else { break };
        let actor = pending.player();
        let action = match actor {
            p if p == PlayerId::P0 => p0.choose(game, p),
            p => p1.choose(game, p),
        };
        if game.step(action).is_err() {
            break;
        }
    }
    game.result()
}
