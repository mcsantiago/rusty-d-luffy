//! Information Set Monte Carlo Tree Search.
//!
//! Plain MCTS assumes the searcher can see the position, which here they
//! cannot. ISMCTS handles that by re-determinizing at the root of every
//! iteration ([`crate::determinize`]) and building one tree over *information
//! sets* rather than states: a node is "the sequence of actions taken from the
//! root", and its statistics pool across every determinization that reached it.
//!
//! Because legal actions differ between determinizations, a node's children are
//! created lazily and UCB1 is computed over the actions actually available in
//! the current determinization — the "availability count" correction that makes
//! ISMCTS unbiased.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use op_core::{legal_actions, Action, Game, PlayerId};

use crate::determinize::determinize;
use crate::heuristic::{evaluate, HeuristicAgent};
use crate::Agent;

#[derive(Debug, Clone)]
pub struct IsmctsConfig {
    /// Determinizations to run. The dominant cost and quality knob.
    pub iterations: u32,
    /// How deep a rollout goes before falling back to the evaluation. Games run
    /// long, so rollouts are truncated rather than played to a terminal.
    pub rollout_depth: u32,
    /// UCB1 exploration constant.
    pub exploration: f64,
    pub seed: u64,
}

impl Default for IsmctsConfig {
    fn default() -> IsmctsConfig {
        IsmctsConfig {
            iterations: 400,
            rollout_depth: 60,
            exploration: 1.4,
            seed: 0xD1E5_0BED,
        }
    }
}

#[derive(Debug)]
struct Node {
    /// Times this node was visited.
    visits: f64,
    /// Summed value, from the searching player's seat.
    total: f64,
    /// Times each child action was *available* to be chosen, which is what the
    /// ISMCTS UCB1 correction divides by.
    children: Vec<Child>,
}

#[derive(Debug)]
struct Child {
    action: Action,
    node: usize,
    availability: f64,
}

impl Node {
    fn new() -> Node {
        Node {
            visits: 0.0,
            total: 0.0,
            children: Vec::new(),
        }
    }
}

/// An ISMCTS agent.
pub struct IsmctsAgent {
    pub config: IsmctsConfig,
    rng: StdRng,
}

impl IsmctsAgent {
    pub fn new(config: IsmctsConfig) -> IsmctsAgent {
        let rng = StdRng::seed_from_u64(config.seed);
        IsmctsAgent { config, rng }
    }

    /// Searches and returns the most-visited root action.
    pub fn search(&mut self, game: &Game, player: PlayerId) -> Action {
        let root_legal = legal_actions(game);
        assert!(!root_legal.is_empty(), "asked to act with no legal action");
        if root_legal.len() == 1 {
            return root_legal.into_iter().next().unwrap();
        }

        let mut nodes = vec![Node::new()];

        for _ in 0..self.config.iterations {
            // A fresh determinization each iteration: this is what makes the
            // tree statistics average over the opponent's possible hands rather
            // than exploiting one particular guess.
            let mut sim = game.clone();
            determinize(&mut sim.state, player, &mut self.rng);

            let mut path: Vec<(usize, usize)> = Vec::new();
            let mut current = 0usize;

            // --- selection + expansion ---
            loop {
                if sim.is_over() || sim.pending().is_none() {
                    break;
                }
                let legal = legal_actions(&sim);
                if legal.is_empty() {
                    break;
                }

                // Bump availability for everything legal here, then pick.
                let untried: Vec<Action> = legal
                    .iter()
                    .filter(|a| !nodes[current].children.iter().any(|c| c.action == **a))
                    .cloned()
                    .collect();

                for child in &mut nodes[current].children {
                    if legal.contains(&child.action) {
                        child.availability += 1.0;
                    }
                }

                let chosen_idx = if !untried.is_empty() {
                    // Expand one new action.
                    let action = untried[self.rng.gen_range(0..untried.len())].clone();
                    nodes.push(Node::new());
                    let node = nodes.len() - 1;
                    nodes[current].children.push(Child {
                        action,
                        node,
                        availability: 1.0,
                    });
                    nodes[current].children.len() - 1
                } else {
                    self.best_child(&nodes, current, &legal)
                };

                let action = nodes[current].children[chosen_idx].action.clone();
                let next = nodes[current].children[chosen_idx].node;
                path.push((current, chosen_idx));

                if sim.step(action).is_err() {
                    break;
                }
                current = next;

                // Stop descending once this iteration expanded a fresh node.
                if nodes[current].visits == 0.0 {
                    break;
                }
            }

            // --- rollout ---
            let value = self.rollout(&mut sim, player);

            // --- backpropagation ---
            nodes[0].visits += 1.0;
            nodes[0].total += value;
            for (parent, child_idx) in path {
                let node = nodes[parent].children[child_idx].node;
                nodes[node].visits += 1.0;
                nodes[node].total += value;
            }
        }

        // Most-visited root action, restricted to what is actually legal.
        nodes[0]
            .children
            .iter()
            .filter(|c| root_legal.contains(&c.action))
            .max_by(|a, b| {
                let av = nodes[a.node].visits;
                let bv = nodes[b.node].visits;
                av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|c| c.action.clone())
            .unwrap_or_else(|| root_legal[0].clone())
    }

    /// UCB1 over the children legal in this determinization, with the
    /// availability correction.
    fn best_child(&self, nodes: &[Node], current: usize, legal: &[Action]) -> usize {
        let mut best = 0usize;
        let mut best_score = f64::NEG_INFINITY;

        for (idx, child) in nodes[current].children.iter().enumerate() {
            if !legal.contains(&child.action) {
                continue;
            }
            let node = &nodes[child.node];
            let score = if node.visits == 0.0 {
                f64::INFINITY
            } else {
                let exploit = node.total / node.visits;
                let explore =
                    self.config.exploration * (child.availability.ln() / node.visits).sqrt();
                exploit + explore
            };
            if score > best_score {
                best_score = score;
                best = idx;
            }
        }
        best
    }

    /// Plays out with the heuristic policy, then scores the result.
    ///
    /// Returns a value in roughly [-1, 1] from `player`'s seat.
    fn rollout(&mut self, sim: &mut Game, player: PlayerId) -> f64 {
        let mut policy = HeuristicAgent::with_exploration(
            StdRng::seed_from_u64(self.rng.gen()),
            0.25,
        );

        for _ in 0..self.config.rollout_depth {
            if sim.is_over() {
                break;
            }
            let Some(pending) = sim.pending() else { break };
            let actor = pending.player();
            let action = policy.choose(sim, actor);
            if sim.step(action).is_err() {
                break;
            }
        }

        if let Some(result) = sim.result() {
            return match result.winner() {
                Some(w) if w == player => 1.0,
                Some(_) => -1.0,
                None => 0.0,
            };
        }
        // Truncated: squash the heuristic evaluation into the same range so
        // finished and unfinished rollouts are comparable.
        (evaluate(sim, player) / 120.0).clamp(-0.95, 0.95)
    }
}

impl Agent for IsmctsAgent {
    fn choose(&mut self, game: &Game, player: PlayerId) -> Action {
        self.search(game, player)
    }
}
