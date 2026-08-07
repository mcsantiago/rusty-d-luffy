//! A deterministic rules engine for the One Piece Card Game.
//!
//! Rules citations throughout refer to the official **Comprehensive Rules
//! v1.2.0** (2026-01-16). Anything non-obvious in the implementation should
//! carry the rule number it comes from.
//!
//! # Determinism
//!
//! A game is a pure function of `(GameConfig, seed, [Action])`. Three rules keep
//! it that way, and all three are load-bearing for replay, netcode and RL:
//!
//! 1. All randomness comes from [`state::GameState::rng`], advanced only by
//!    explicit shuffles.
//! 2. No `HashMap`/`HashSet` iteration in rules paths — ordered containers and
//!    dense integer ids only.
//! 3. No floating point anywhere in the rules.
//!
//! # Hidden information
//!
//! [`state::GameState`] is omniscient. Clients and imperfect-information agents
//! see [`view::PlayerView`] instead.

pub mod action;
pub mod card;
pub mod derive;
pub mod effect;
pub mod event;
pub mod game;
pub mod ids;
pub mod legal;
pub mod replay;
pub mod script;
pub mod state;
pub mod view;
pub mod zone;

pub use action::{Action, IllegalAction, Pending};
pub use card::{CardDb, CardDef, Category, Color, Keyword};
pub use derive::{Characteristics, Derived};
pub use event::{CardRef, GameEvent, PlayerEvent};
pub use game::{DeckList, Game, GameConfig, PlayerOutcome, SetupError, StepOutcome};
pub use ids::{CardDefId, CardInstanceId, PlayerId};
pub use legal::legal_actions;
pub use replay::SessionLog;
pub use script::{CardScript, NoScripts, ScriptSource};
pub use state::{BattleStep, GameOver, GameState, Phase};
pub use view::PlayerView;
pub use zone::Zone;
