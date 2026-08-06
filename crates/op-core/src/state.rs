//! Game state.
//!
//! `GameState` is the omniscient truth and is never sent to a client — see
//! [`crate::view::PlayerView`] for the redacted projection.

use std::collections::BTreeMap;

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::card::{CardDb, Category};
use crate::effect::{Modifier, ResolutionStack};
use crate::ids::{CardDefId, CardInstanceId, PlayerId};
use crate::zone::Zone;

/// Turn phases (6-1-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Phase {
    Refresh,
    Draw,
    Don,
    Main,
    End,
}

/// The five battle steps (7-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BattleStep {
    Attack,
    Block,
    Counter,
    Damage,
    EndOfBattle,
}

/// An in-progress battle (7-1). Present only between attack declaration and
/// end of battle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BattleState {
    pub step: BattleStep,
    pub attacker: CardInstanceId,
    /// The card currently being attacked. A successful `[Blocker]` replaces
    /// this with the blocker (10-1-4-1).
    pub target: CardInstanceId,
    /// The originally declared target, before any blocker redirect.
    pub original_target: CardInstanceId,
    /// Set once a `[Blocker]` has been used; only one per battle (7-1-2-1).
    pub blocker_used: bool,
    /// Zones the attacker and target occupied at the start of the current step,
    /// for the "has it changed areas?" check run between steps (7-1-1-4 etc).
    pub attacker_zone: Zone,
    pub target_zone: Zone,
}

/// A single physical card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardInstance {
    pub id: CardInstanceId,
    pub def: CardDefId,
    /// Never changes; determines whose trash/deck the card returns to.
    pub owner: PlayerId,
    /// Whose area the card currently sits in.
    pub controller: PlayerId,
    pub zone: Zone,
    pub rested: bool,
    /// DON!! cards given to this card (6-5-5-1).
    pub attached_don: Vec<CardInstanceId>,
    /// The turn number this card entered the Character area, for summoning
    /// sickness. `None` if it is not in play.
    pub played_on_turn: Option<u32>,
    /// Effects that have declared themselves used this turn, keyed by an
    /// effect slot index on the card ([Once Per Turn], 10-2-13).
    pub used_once_per_turn: Vec<u8>,
}

impl CardInstance {
    pub fn is_active(&self) -> bool {
        !self.rested
    }
}

/// Per-player areas. Each `Vec` is the ordered contents of one area; order is
/// meaningful for Deck, DON!! deck, Life and Trash (3-1-7).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerState {
    pub deck: Vec<CardInstanceId>,
    pub don_deck: Vec<CardInstanceId>,
    pub hand: Vec<CardInstanceId>,
    pub trash: Vec<CardInstanceId>,
    /// Exactly one card, and it never leaves (3-6-3).
    pub leader: Option<CardInstanceId>,
    /// At most 5 (3-7-6).
    pub characters: Vec<CardInstanceId>,
    /// At most 1 (3-8-5).
    pub stage: Option<CardInstanceId>,
    pub cost_area: Vec<CardInstanceId>,
    /// Index 0 is the top of the Life area — the card taken on damage.
    pub life: Vec<CardInstanceId>,
    /// Whether this player has already taken their one mulligan (5-2-1-6).
    pub mulliganed: bool,
}

impl PlayerState {
    pub fn zone(&self, zone: Zone) -> &[CardInstanceId] {
        match zone {
            Zone::Deck => &self.deck,
            Zone::DonDeck => &self.don_deck,
            Zone::Hand => &self.hand,
            Zone::Trash => &self.trash,
            Zone::Leader => self.leader.as_slice(),
            Zone::Character => &self.characters,
            Zone::Stage => self.stage.as_slice(),
            Zone::Cost => &self.cost_area,
            Zone::Life => &self.life,
            Zone::Limbo => &[],
        }
    }

    fn zone_mut(&mut self, zone: Zone) -> Option<&mut Vec<CardInstanceId>> {
        Some(match zone {
            Zone::Deck => &mut self.deck,
            Zone::DonDeck => &mut self.don_deck,
            Zone::Hand => &mut self.hand,
            Zone::Trash => &mut self.trash,
            Zone::Character => &mut self.characters,
            Zone::Cost => &mut self.cost_area,
            Zone::Life => &mut self.life,
            Zone::Leader | Zone::Stage | Zone::Limbo => return None,
        })
    }

    /// Active DON!! in the cost area — the pool available to pay costs.
    pub fn active_don(&self, state: &GameState) -> usize {
        self.cost_area
            .iter()
            .filter(|&&id| state.card(id).is_active())
            .count()
    }
}

/// Damage being dealt to a Leader, one point at a time.
///
/// `[Double Attack]` deals 2 (10-1-2), and each point is resolved separately
/// because a `[Trigger]` on the revealed life card suspends processing
/// (8-6-2-1) — so the remaining count has to survive the suspension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DamageState {
    /// The player taking damage.
    pub player: PlayerId,
    pub remaining: u8,
    /// Life cards go to the trash and skip their `[Trigger]` (10-1-3).
    pub banish: bool,
}

/// Why the game ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameOver {
    /// Leader took damage at 0 Life (9-2-1-1).
    LifeDepleted { loser: PlayerId },
    /// Deck ran out (9-2-1-2).
    DeckOut { loser: PlayerId },
    /// Both players met a defeat condition simultaneously (9-2-1).
    Draw,
}

impl GameOver {
    pub fn winner(self) -> Option<PlayerId> {
        match self {
            GameOver::LifeDepleted { loser } | GameOver::DeckOut { loser } => {
                Some(loser.opponent())
            }
            GameOver::Draw => None,
        }
    }
}

/// The complete, omniscient game state.
///
/// Cloning this is the primitive MCTS and the server both rely on, so it is
/// plain data throughout — no trait objects, no interior mutability, no
/// references into the card database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameState {
    /// Indexed by `CardInstanceId`; never reordered or removed from.
    pub cards: Vec<CardInstance>,
    pub players: [PlayerState; 2],

    pub turn: u32,
    pub turn_player: PlayerId,
    pub phase: Phase,
    /// The player who went first, who skips their first draw (6-3-1) and places
    /// only one DON!! on their first turn (6-4-1).
    pub first_player: PlayerId,

    pub battle: Option<BattleState>,
    /// In-progress damage to a Leader, suspended while the damaged player
    /// decides about a `[Trigger]` (8-6-2-1).
    pub damage: Option<DamageState>,
    /// The decision the engine is waiting on. A suspended decision point is
    /// part of the position, so it lives in the state.
    pub pending: Option<crate::action::Pending>,
    /// Suspended effect resolution (see [`crate::effect`]).
    pub stack: ResolutionStack,
    /// Temporary modifiers with a duration; permanent effects are derived, not
    /// stored (8-1-3-3).
    pub modifiers: Vec<Modifier>,

    pub game_over: Option<GameOver>,

    /// Set once the End Phase has queued its end-of-turn effects, so the phase
    /// can wait for them to resolve before tearing down turn-scoped state
    /// (6-6-1-1 must complete before 6-6-1-3).
    pub end_autos_queued: bool,

    /// Single deterministic randomness source. Advanced only through explicit
    /// shuffle/sample calls.
    pub rng: ChaCha8Rng,
    /// Bumped on every mutation, so derived characteristics can be memoized.
    pub version: u64,
}

impl GameState {
    pub fn new(seed: u64, first_player: PlayerId) -> GameState {
        GameState {
            cards: Vec::new(),
            players: [PlayerState::default(), PlayerState::default()],
            turn: 0,
            turn_player: first_player,
            phase: Phase::Refresh,
            first_player,
            battle: None,
            damage: None,
            pending: None,
            stack: ResolutionStack::default(),
            modifiers: Vec::new(),
            game_over: None,
            end_autos_queued: false,
            rng: ChaCha8Rng::seed_from_u64(seed),
            version: 0,
        }
    }

    pub fn card(&self, id: CardInstanceId) -> &CardInstance {
        &self.cards[id.index()]
    }

    pub fn card_mut(&mut self, id: CardInstanceId) -> &mut CardInstance {
        self.version += 1;
        &mut self.cards[id.index()]
    }

    pub fn player(&self, p: PlayerId) -> &PlayerState {
        &self.players[p.index()]
    }

    pub fn player_mut(&mut self, p: PlayerId) -> &mut PlayerState {
        self.version += 1;
        &mut self.players[p.index()]
    }

    pub fn is_turn_player(&self, p: PlayerId) -> bool {
        self.turn_player == p
    }

    /// Creates a card instance in the given area.
    pub fn spawn(
        &mut self,
        def: CardDefId,
        owner: PlayerId,
        zone: Zone,
    ) -> CardInstanceId {
        let id = CardInstanceId(self.cards.len() as u32);
        self.cards.push(CardInstance {
            id,
            def,
            owner,
            controller: owner,
            zone: Zone::Limbo,
            rested: false,
            attached_don: Vec::new(),
            played_on_turn: None,
            used_once_per_turn: Vec::new(),
        });
        self.put(id, owner, zone, Placement::Bottom);
        id
    }

    /// Removes `id` from whatever area it is in, leaving it in `Limbo`.
    pub fn lift(&mut self, id: CardInstanceId) {
        let (controller, zone) = {
            let c = self.card(id);
            (c.controller, c.zone)
        };
        let ps = self.player_mut(controller);
        match zone {
            Zone::Leader => {
                if ps.leader == Some(id) {
                    ps.leader = None;
                }
            }
            Zone::Stage => {
                if ps.stage == Some(id) {
                    ps.stage = None;
                }
            }
            Zone::Limbo => {}
            other => {
                if let Some(v) = ps.zone_mut(other) {
                    v.retain(|&x| x != id);
                }
            }
        }
        self.card_mut(id).zone = Zone::Limbo;
    }

    /// Places a card into an area. The caller is responsible for having lifted
    /// it out of its previous area first (or for it being freshly spawned).
    pub fn put(
        &mut self,
        id: CardInstanceId,
        controller: PlayerId,
        zone: Zone,
        placement: Placement,
    ) {
        {
            let c = self.card_mut(id);
            c.controller = controller;
            c.zone = zone;
        }
        let ps = self.player_mut(controller);
        match zone {
            Zone::Leader => ps.leader = Some(id),
            Zone::Stage => ps.stage = Some(id),
            Zone::Limbo => {}
            other => {
                if let Some(v) = ps.zone_mut(other) {
                    match placement {
                        Placement::Top => v.insert(0, id),
                        Placement::Bottom => v.push(id),
                    }
                }
            }
        }
    }

    /// Moves a card between areas.
    ///
    /// Per 3-1-6 a card leaving the Character or Stage area is a new object:
    /// modifiers applied to it are dropped, and any DON!! given to it returns
    /// to the cost area rested (6-5-5-4). Both are handled here so no caller can
    /// forget.
    pub fn move_card(
        &mut self,
        id: CardInstanceId,
        to_controller: PlayerId,
        to: Zone,
        placement: Placement,
    ) {
        let from = self.card(id).zone;
        self.detach_don(id);
        self.lift(id);
        self.put(id, to_controller, to, placement);

        if matches!(from, Zone::Character | Zone::Stage) && from != to {
            self.modifiers.retain(|m| m.target != id);
            let c = self.card_mut(id);
            c.rested = false;
            c.played_on_turn = None;
            c.used_once_per_turn.clear();
        }
    }

    /// Returns every DON!! given to `id` to its owner's cost area, rested
    /// (6-5-5-4).
    pub fn detach_don(&mut self, id: CardInstanceId) {
        let dons = std::mem::take(&mut self.card_mut(id).attached_don);
        for don in dons {
            let owner = self.card(don).owner;
            self.lift(don);
            self.put(don, owner, Zone::Cost, Placement::Bottom);
            self.card_mut(don).rested = true;
        }
    }

    /// Cards in the Leader and Character areas of `p` — everything that can
    /// attack or be given DON!!.
    pub fn battlers(&self, p: PlayerId) -> Vec<CardInstanceId> {
        let ps = self.player(p);
        ps.leader.iter().chain(ps.characters.iter()).copied().collect()
    }

    /// Every card in play for either player, turn player first (the order most
    /// rules processes require, e.g. 8-6-1).
    pub fn all_in_play(&self) -> Vec<CardInstanceId> {
        let mut out = Vec::new();
        for p in [self.turn_player, self.turn_player.opponent()] {
            let ps = self.player(p);
            out.extend(ps.leader.iter().copied());
            out.extend(ps.characters.iter().copied());
            out.extend(ps.stage.iter().copied());
        }
        out
    }

    pub fn def<'a>(&self, db: &'a CardDb, id: CardInstanceId) -> &'a crate::card::CardDef {
        db.get(self.card(id).def)
    }

    pub fn category(&self, db: &CardDb, id: CardInstanceId) -> Category {
        self.def(db, id).category
    }

    /// A structural hash of everything that affects play. Used by the
    /// determinism tests; deliberately excludes nothing that the rules read.
    pub fn state_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for c in &self.cards {
            c.id.hash(&mut h);
            c.def.hash(&mut h);
            c.owner.hash(&mut h);
            c.controller.hash(&mut h);
            c.zone.hash(&mut h);
            c.rested.hash(&mut h);
            c.attached_don.hash(&mut h);
            c.played_on_turn.hash(&mut h);
            c.used_once_per_turn.hash(&mut h);
        }
        for p in &self.players {
            p.deck.hash(&mut h);
            p.don_deck.hash(&mut h);
            p.hand.hash(&mut h);
            p.trash.hash(&mut h);
            p.leader.hash(&mut h);
            p.characters.hash(&mut h);
            p.stage.hash(&mut h);
            p.cost_area.hash(&mut h);
            p.life.hash(&mut h);
        }
        self.turn.hash(&mut h);
        self.turn_player.hash(&mut h);
        self.phase.hash(&mut h);
        self.battle.hash(&mut h);
        self.damage.hash(&mut h);
        self.modifiers.hash(&mut h);
        self.game_over.is_some().hash(&mut h);
        // The pending decision is part of the position; hash its discriminant
        // and owner rather than deriving Hash across the whole payload.
        match &self.pending {
            None => 0u8.hash(&mut h),
            Some(p) => {
                1u8.hash(&mut h);
                std::mem::discriminant(p).hash(&mut h);
                p.player().hash(&mut h);
            }
        }
        h.finish()
    }

    /// Per-card scratch space used by rules processes that need to note
    /// something transient without widening `CardInstance`.
    pub fn count_in_zone(&self, p: PlayerId, zone: Zone) -> usize {
        self.player(p).zone(zone).len()
    }
}

/// Where in an ordered area a card is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Top,
    Bottom,
}

/// Counts of things effects commonly ask about, cached per resolution.
pub type Counters = BTreeMap<String, i32>;
