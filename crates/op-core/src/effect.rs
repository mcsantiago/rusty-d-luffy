//! Effects, modifiers, and suspended resolution.
//!
//! The One Piece Card Game has no MTG-style stack: effects resolve immediately
//! and in full, turn player first (8-6-1). What it does have is effects that
//! stop half-way to ask a player a question — choose a target (8-4-4), activate
//! a `[Trigger]` mid-damage (8-6-2-1), pay a cost (8-4-1-3).
//!
//! That suspension is represented as plain data ([`EffectFrame::ip`]) rather
//! than a coroutine, so `GameState` stays `Clone + Serialize` even mid-effect —
//! which is what MCTS cloning, server snapshots and replay all require.

use serde::{Deserialize, Serialize};

use crate::card::Keyword;
use crate::ids::{CardInstanceId, PlayerId};
use crate::zone::Zone;

/// How long a temporary modifier lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Duration {
    /// Ends at end of battle (7-1-5-3/4).
    ThisBattle,
    /// Ends in the End Phase (6-6-1-3).
    ThisTurn,
    /// Ends in the End Phase of this player's next turn.
    UntilYourNextTurn,
}

/// What a modifier does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModKind {
    Power(i32),
    /// Cost modification, e.g. "give -4 cost during this turn". Clamped at 0
    /// when derived; a card's cost never goes negative.
    Cost(i32),
    /// "This Character cannot be K.O.'d by effects." Losing a battle still
    /// K.O.s it (10-2-1-1) — only effect-driven K.O. is prevented.
    CannotBeKoedByEffect,
    /// "This Character cannot be K.O.'d in battle by Leaders" (ST08-002). The
    /// complement of [`ModKind::CannotBeKoedByEffect`]: this one stops the
    /// battle K.O. of 7-1-4-1-2, and only when the attacker is a Leader.
    CannotBeKoedInBattleByLeader,
    GrantKeyword(Keyword),
    /// The opponent cannot activate `[Blocker]` against this card's attack
    /// (ST01-012, ST01-016).
    CannotBeBlocked,
    /// The opponent cannot block this attack with a `[Blocker]` whose power is
    /// at or above the given threshold (ST01-002).
    BlockerPowerCeiling(i32),
}

/// A temporary, duration-bound change to a card.
///
/// Permanent effects (8-1-3-3) are *not* stored here — they are recomputed from
/// the cards in play on every derive, so that they layer correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Modifier {
    pub target: CardInstanceId,
    pub kind: ModKind,
    pub duration: Duration,
    pub source: CardInstanceId,
    /// Whose effect this is, for turn-based expiry of `UntilYourNextTurn`.
    pub controller: PlayerId,
}

/// Timings at which auto effects activate (8-1-3-1-1, 10-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Timing {
    OnPlay,
    WhenAttacking,
    OnYourOpponentsAttack,
    OnBlock,
    OnKo,
    /// "When a Character is K.O.'d" — fires on every card in play, for *any*
    /// Character leaving via a K.O., either player's (ST08-001). Distinct from
    /// [`Timing::OnKo`], which is the card's own K.O.
    OnCharacterKoed,
    EndOfYourTurn,
    EndOfYourOpponentsTurn,
    Trigger,
    /// End of a battle this card took part in. Not a printed keyword, but the
    /// timing "if this Character battles your opponent's Character…" needs
    /// (ST02-010).
    EndOfBattle,
}

impl Timing {
    /// Whether the engine ever activates an auto effect at this timing.
    ///
    /// A script declaring a timing nothing fires is dead: the card simply has
    /// no text. Every `true` below must correspond to a `queue_autos` call in
    /// `game.rs`, and `validate` turns that correspondence into a test.
    pub fn is_activated_by_engine(self) -> bool {
        match self {
            Timing::OnPlay
            | Timing::WhenAttacking
            | Timing::OnYourOpponentsAttack
            | Timing::OnCharacterKoed
            | Timing::EndOfYourTurn
            | Timing::EndOfYourOpponentsTurn
            | Timing::EndOfBattle => true,
            // Declared for the printed keyword, not yet wired: blocking is
            // resolved from the `Blocker` keyword, without consulting scripts.
            Timing::OnBlock => false,
            // No K.O. hook exists; `knock_out` moves the card and stops.
            Timing::OnKo => false,
            // `[Trigger]` ops live in `CardScript::trigger` and are resolved
            // from the Life area (10-1-5-3); the variant is never queued.
            Timing::Trigger => false,
        }
    }
}

/// Which DON!! in the cost area an effect may give.
///
/// A card reading "give up to 1 rested DON!! card" qualifies the DON!! being
/// *selected*, not the state it ends up in. Bandai's ST01-001 ruling settles
/// it: a DON!! already given to another Character may not be taken, on the
/// grounds that it is not a rested DON!! card.
///
/// Giving therefore never changes a DON!!'s rest state. They are rested on the
/// way back to the cost area instead, by 6-2-3 and 6-5-5-4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DonSource {
    /// The ordinary give of 6-5-5-1.
    Active,
    /// Only DON!! already rested, as ST01-001 and friends require.
    Rested,
    /// Either, for a card that qualifies neither way.
    Any,
}

impl DonSource {
    pub fn admits(self, rested: bool) -> bool {
        match self {
            DonSource::Active => !rested,
            DonSource::Rested => rested,
            DonSource::Any => true,
        }
    }
}

/// One instruction in a card script.
///
/// The vocabulary grows as sets are implemented; the kernel only needs to know
/// how to execute an op and when an op needs to suspend for a choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectOp {
    /// Ask the controller to pick cards matching `select`, binding the result.
    Choose { key: String, select: Selector },
    /// Bind *every* card matching `select`, with no decision to make. "K.O.
    /// **all** Characters with a cost of 1 or less" (ST08-005) is not a choice,
    /// and offering it as one would put a pointless subset enumeration in front
    /// of both the player and the search.
    SelectAll { key: String, select: Selector },
    /// Ask the controller to pick up to `up_to` of the cards already bound
    /// under `from`, binding the answer under `key`.
    ///
    /// Exists because some pools cannot be re-derived from the state at choice
    /// time. `[BATTLED]` is the case in hand: the battle is over by the time an
    /// `EndOfBattle` effect resolves, so the participants have to travel with
    /// the frame (ST08-013).
    ChooseFrom {
        key: String,
        from: String,
        up_to: u8,
    },
    /// Grant a modifier to every card bound under `key`.
    Modify {
        key: String,
        kind: ModKind,
        duration: Duration,
    },
    /// K.O. every card bound under `key` (10-2-1).
    Ko { key: String },
    /// Rest every card bound under `key`.
    Rest { key: String },
    /// Set every card bound under `key` active.
    SetActive { key: String },
    /// Draw `n` cards.
    Draw { player: Who, n: u8 },
    /// Give `n` DON!! from the cost area to the card bound under `key`
    /// (6-5-5-1), choosing them per [`DonSource`].
    GiveDon {
        key: String,
        n: u8,
        source: DonSource,
    },
    /// Move every card bound under `key` to `to`.
    MoveTo { key: String, to: Zone },
    /// Play every card bound under `key` from wherever it is, for free.
    /// `[Trigger] Play this card.` and similar.
    PlayBound { key: String },
    /// Look at the top `n` cards of your deck, let the controller reveal up to
    /// `up_to` matching `filters` and add them to hand, then put the rest on the
    /// bottom of the deck (ST02-007).
    ///
    /// The rest go to the bottom in the order they were drawn. The card text
    /// permits any order, but the ordering is never strategically meaningful
    /// against a deck whose order is unknown, and enumerating permutations
    /// would blow up the action space for search and RL.
    DigTop {
        n: u8,
        key: String,
        up_to: u8,
        filters: Vec<Filter>,
    },
    /// Stop resolving unless the condition holds (8-3-3 "if" clauses).
    RequireIf { cond: Condition },
    /// Stop resolving unless `key` bound at least one card — the "If you do,"
    /// half of "you may X. If you do, Y." (ST08-013), where the optionality of
    /// X is a `Choose` the player may answer with nothing.
    RequireBound { key: String },
    /// Add `n` DON!! cards from the DON!! deck to the cost area (ST04-008 and
    /// friends). `rested` decides which way up they arrive; "set it as active"
    /// makes the DON!! spendable this turn, which is the whole point of the
    /// purple deck.
    AddDon { n: u8, rested: bool },
    /// Trash `n` Life cards from the top of a player's Life area (ST04-001).
    ///
    /// No choice is offered, and that is deliberate rather than lazy. Life is a
    /// secret area (3-1-4): the cards are face down and indistinguishable, so
    /// picking among them decides nothing — while enumerating them as options
    /// would put their `CardInstanceId`s in front of the choosing player and
    /// leak the contents to anyone holding the decklist.
    TrashLife { player: Who, n: u8 },
    /// Trash the effect's source if it is still in no area. Appended by the
    /// engine after a `[Trigger]`'s ops so that a Trigger which played or
    /// otherwise relocated the card does not then trash it (10-1-5-3).
    TrashIfInLimbo,
}

impl EffectOp {
    /// The op's name, for diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            EffectOp::Choose { .. } => "Choose",
            EffectOp::SelectAll { .. } => "SelectAll",
            EffectOp::ChooseFrom { .. } => "ChooseFrom",
            EffectOp::Modify { .. } => "Modify",
            EffectOp::Ko { .. } => "Ko",
            EffectOp::Rest { .. } => "Rest",
            EffectOp::SetActive { .. } => "SetActive",
            EffectOp::Draw { .. } => "Draw",
            EffectOp::GiveDon { .. } => "GiveDon",
            EffectOp::MoveTo { .. } => "MoveTo",
            EffectOp::PlayBound { .. } => "PlayBound",
            EffectOp::DigTop { .. } => "DigTop",
            EffectOp::RequireIf { .. } => "RequireIf",
            EffectOp::AddDon { .. } => "AddDon",
            EffectOp::TrashLife { .. } => "TrashLife",
            EffectOp::TrashIfInLimbo => "TrashIfInLimbo",
            EffectOp::RequireBound { .. } => "RequireBound",
        }
    }

    /// The binding key this op acts on, if any.
    ///
    /// Reading a key nothing bound is not an error at run time — [`EffectFrame::bound`]
    /// hands back an empty slice and the op does nothing — so this exists for
    /// [`crate::validate`] to catch the mistake before a game does.
    ///
    /// [`EffectFrame::bound`]: EffectFrame::bound
    pub fn reads(&self) -> Option<&str> {
        match self {
            EffectOp::Modify { key, .. }
            | EffectOp::Ko { key }
            | EffectOp::Rest { key }
            | EffectOp::SetActive { key }
            | EffectOp::GiveDon { key, .. }
            | EffectOp::MoveTo { key, .. }
            | EffectOp::RequireBound { key }
            | EffectOp::ChooseFrom { from: key, .. }
            | EffectOp::PlayBound { key } => Some(key),
            EffectOp::Choose { .. }
            | EffectOp::SelectAll { .. }
            | EffectOp::DigTop { .. }
            | EffectOp::Draw { .. }
            | EffectOp::AddDon { .. }
            | EffectOp::TrashLife { .. }
            | EffectOp::RequireIf { .. }
            | EffectOp::TrashIfInLimbo => None,
        }
    }

    /// The binding key this op fills in, if any.
    pub fn binds(&self) -> Option<&str> {
        match self {
            EffectOp::Choose { key, .. }
            | EffectOp::SelectAll { key, .. }
            | EffectOp::ChooseFrom { key, .. }
            | EffectOp::DigTop { key, .. } => Some(key),
            EffectOp::Modify { .. }
            | EffectOp::Ko { .. }
            | EffectOp::Rest { .. }
            | EffectOp::SetActive { .. }
            | EffectOp::Draw { .. }
            | EffectOp::GiveDon { .. }
            | EffectOp::MoveTo { .. }
            | EffectOp::PlayBound { .. }
            | EffectOp::AddDon { .. }
            | EffectOp::TrashLife { .. }
            | EffectOp::RequireIf { .. }
            | EffectOp::RequireBound { .. }
            | EffectOp::TrashIfInLimbo => None,
        }
    }
}

/// Who an op applies to, relative to the effect's controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Who {
    You,
    Opponent,
    /// Both players. Card text that says plain "Characters" rather than "your"
    /// or "your opponent's" reaches the whole board (ST08-005).
    Both,
}

/// A filtered request for cards, resolved against the state at choice time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    pub zone: Zone,
    pub owner: Who,
    /// `Some(n)` means "up to n"; the player may always choose fewer, including
    /// zero, when the text says "up to" (8-4-4-1).
    pub up_to: u8,
    /// Fewest cards the player must name. 0 — the default, and what "up to"
    /// means — lets them decline entirely. Set it where the text instructs
    /// rather than offers: "trash 1 card from your hand" (ST04-005).
    pub at_least: u8,
    pub filters: Vec<Filter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Filter {
    CostAtMost(u8),
    PowerAtMost(i32),
    /// Matches if the card has any one of these types ("your {Supernovas} or
    /// {Heart Pirates} type Characters").
    HasAnyType(Vec<String>),
    HasKeyword(Keyword),
    IsRested(bool),
    /// Excludes the card the effect originated from ("other than this card").
    NotSelf,
    /// Restricts to a card category, e.g. Characters only.
    IsCategory(crate::card::Category),
    /// Matches a card of this colour ("1 **black** Character card", ST08-014's
    /// `[Trigger]`). A card may be multi-coloured, so this is "has", not "is".
    HasColor(crate::card::Color),
    /// Matches a card by printed name, e.g. "1 [Page One] card" (ST04-002).
    /// Names, not card numbers: several numbers share a name and the text means
    /// any of them (2-14-3).
    HasName(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Condition {
    /// `[DON!! xN]` — at least N DON!! given to the source card (10-2-9).
    DonAttached(u8),
    /// `[Your Turn]` (10-2-11).
    YourTurn,
    /// `[Opponent's Turn]` (10-2-12).
    OpponentsTurn,
    /// "If you have N or more Characters".
    CharacterCountAtLeast(u8),
    /// "If this Character is rested".
    SelfRested,
    /// "If your Leader has the {Type} type".
    LeaderHasType(String),
    /// "If there is a Character with a cost of N" — either player's.
    AnyCharacterWithCost(u8),
}

/// A suspended effect in mid-resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectFrame {
    /// The card whose text this is. "This card" in the text refers to it.
    pub source: CardInstanceId,
    pub controller: PlayerId,
    pub ops: Vec<EffectOp>,
    /// Instruction pointer — where to resume after a suspension.
    pub ip: usize,
    /// Cards chosen so far, by binding key.
    pub bindings: Vec<(String, Vec<CardInstanceId>)>,
}

/// Binding key always pre-bound to the card the effect came from, so ops can
/// refer to "this card" without a selector.
pub const SELF_BINDING: &str = "self";

impl EffectFrame {
    pub fn new(source: CardInstanceId, controller: PlayerId, ops: Vec<EffectOp>) -> EffectFrame {
        EffectFrame {
            source,
            controller,
            ops,
            ip: 0,
            bindings: vec![(SELF_BINDING.to_string(), vec![source])],
        }
    }

    pub fn bind(&mut self, key: &str, cards: Vec<CardInstanceId>) {
        if let Some(slot) = self.bindings.iter_mut().find(|(k, _)| k == key) {
            slot.1 = cards;
        } else {
            self.bindings.push((key.to_string(), cards));
        }
    }

    pub fn bound(&self, key: &str) -> &[CardInstanceId] {
        self.bindings
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[])
    }

    /// Whether `key` has been answered. Distinct from an empty binding, which
    /// is a legitimate answer to an "up to" choice (8-4-4-1).
    pub fn has_binding(&self, key: &str) -> bool {
        self.bindings.iter().any(|(k, _)| k == key)
    }
}

/// Effects awaiting resolution. Frames resolve last-in-first-out: an effect
/// whose timing is met during another effect's resolution resolves after the
/// current one finishes (8-6-3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionStack {
    pub frames: Vec<EffectFrame>,
}

impl ResolutionStack {
    pub fn push(&mut self, frame: EffectFrame) {
        self.frames.push(frame);
    }

    pub fn top(&self) -> Option<&EffectFrame> {
        self.frames.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut EffectFrame> {
        self.frames.last_mut()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}
