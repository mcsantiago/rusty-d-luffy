//! A hand-written evaluation and greedy agent.
//!
//! Serves two purposes: a baseline opponent, and the rollout policy for
//! [`crate::ismcts`] — uniform-random rollouts in a game this long produce very
//! noisy value estimates, so the search leans on this instead.

use rand::Rng;

use op_core::card::Category;
use op_core::{legal_actions, Action, Game, PlayerId};

use crate::Agent;

/// Position evaluation from `player`'s seat, in arbitrary units.
///
/// Weights are deliberately simple and readable: Life is the win condition and
/// dominates, board presence wins future turns, cards in hand are optionality.
pub fn evaluate(game: &Game, player: PlayerId) -> f64 {
    if let Some(result) = game.result() {
        return match result.winner() {
            Some(w) if w == player => 1000.0,
            Some(_) => -1000.0,
            None => 0.0,
        };
    }

    let derived = game.derived();
    let mut score = 0.0;

    for (seat, sign) in [(player, 1.0), (player.opponent(), -1.0)] {
        let ps = game.state.player(seat);

        // Life is the clock; each point is worth far more than a card.
        score += sign * 22.0 * ps.life.len() as f64;
        score += sign * 4.0 * ps.hand.len() as f64;

        for &character in &ps.characters {
            // Bodies matter, and bigger bodies matter more. Power is scaled
            // down so a 5000-power body is worth roughly one and a half cards.
            score += sign * 3.0;
            score += sign * derived.power(character) as f64 / 1500.0;
        }

        // Unspent DON!! is wasted tempo, but available DON!! on the opponent's
        // turn is counter mana, so it is only lightly weighted.
        score += sign * 0.5 * ps.cost_area.len() as f64;
    }

    score
}

/// Picks the action with the best immediate evaluation, breaking ties randomly.
///
/// It looks exactly one action ahead, so it plays reasonable cards and takes
/// good attacks but has no plan. Good enough to be a baseline and to keep MCTS
/// rollouts sane.
pub struct HeuristicAgent<R: Rng> {
    pub rng: R,
    /// Chance of playing a uniformly random legal action instead. Keeps rollout
    /// lines from collapsing onto one deterministic path.
    pub exploration: f64,
}

impl<R: Rng> HeuristicAgent<R> {
    pub fn new(rng: R) -> HeuristicAgent<R> {
        HeuristicAgent {
            rng,
            exploration: 0.0,
        }
    }

    pub fn with_exploration(rng: R, exploration: f64) -> HeuristicAgent<R> {
        HeuristicAgent { rng, exploration }
    }
}

impl<R: Rng> Agent for HeuristicAgent<R> {
    fn choose(&mut self, game: &Game, player: PlayerId) -> Action {
        let legal = legal_actions(game);
        assert!(!legal.is_empty(), "asked to act with no legal action");

        if self.exploration > 0.0 && self.rng.gen_bool(self.exploration) {
            return legal[self.rng.gen_range(0..legal.len())].clone();
        }

        let mut best = Vec::new();
        let mut best_score = f64::NEG_INFINITY;

        for action in &legal {
            let mut probe = game.clone();
            if probe.step(action.clone()).is_err() {
                continue;
            }
            let score = evaluate(&probe, player) + shape(action, game);
            if score > best_score + 1e-9 {
                best_score = score;
                best.clear();
                best.push(action.clone());
            } else if (score - best_score).abs() <= 1e-9 {
                best.push(action.clone());
            }
        }

        if best.is_empty() {
            return legal[self.rng.gen_range(0..legal.len())].clone();
        }
        best[self.rng.gen_range(0..best.len())].clone()
    }
}

/// Nudges for choices whose value the position evaluation cannot see one ply
/// out.
fn shape(action: &Action, game: &Game) -> f64 {
    match action {
        // Ending the Main Phase should lose to any action with real upside, so
        // that the agent actually develops its board.
        Action::EndMainPhase => -0.75,
        // Attaching DON!! is invisible to the evaluation until it is used, but
        // it is almost always right to spend it on a threat before attacking.
        Action::GiveDon { .. } => 0.6,
        // Keeping the mulligan is fine; taking one is only worth it on a hand
        // with nothing cheap, which one-ply search cannot judge.
        Action::Mulligan(true) => -0.1,
        Action::PlayCard { card } => {
            // Prefer developing Characters over spending Events early.
            match game.db().get(game.state.card(*card).def).category {
                Category::Character => 0.5,
                _ => 0.0,
            }
        }
        _ => 0.0,
    }
}
