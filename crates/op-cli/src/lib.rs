//! Shared pieces of the terminal client.
//!
//! A library only so that `onepiece` and `op-replay` can agree on where card
//! data and session logs live; the gameplay loop stays in the binary.

pub mod data;
pub mod decks;
