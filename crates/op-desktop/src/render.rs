//! Turning engine types into strings for the UI.
//!
//! Event rendering takes a [`PlayerEvent`], so a card the viewer may not
//! identify has no id to look up and cannot be named by accident.

use op_core::{Action, CardRef, Game, Pending, PlayerEvent, PlayerId};

pub fn line(event: &PlayerEvent, game: &Game, viewer: PlayerId) -> Option<String> {
    use PlayerEvent as E;
    let db = game.db();
    let name = |card: CardRef| match card.id() {
        Some(id) => db.get(game.state.card(id).def).name.clone(),
        None => "a card".to_string(),
    };
    let who = |p: PlayerId| if p == viewer { "You" } else { "Opponent" };

    Some(match event {
        E::TurnStarted { turn, player } => format!("— Turn {turn}, {} —", who(*player)),
        E::Drew { player, card } => match card.id() {
            Some(_) => format!("You drew {}", name(*card)),
            None => format!("{} drew a card", who(*player)),
        },
        E::CardPlayed { player, card, .. } => {
            format!("{} played {}", who(*player), name(*card))
        }
        E::DonGiven { player, to, .. } => {
            format!("{} gave DON!! to {}", who(*player), name(*to))
        }
        E::AttackDeclared { attacker, target } => {
            format!("{} attacks {}", name(*attacker), name(*target))
        }
        E::Blocked { blocker, .. } => format!("{} blocks", name(*blocker)),
        E::Countered {
            player,
            target,
            amount,
            ..
        } => format!("{} countered: {} +{amount}", who(*player), name(*target)),
        E::BattleResolved {
            attacker_power,
            target_power,
            attacker_won,
            ..
        } => format!(
            "Battle {attacker_power} vs {target_power} — {}",
            if *attacker_won {
                "attacker wins"
            } else {
                "attack repelled"
            }
        ),
        E::KnockedOut { card } => format!("{} was K.O.'d", name(*card)),
        E::DamageDealt { player, amount } => {
            format!("{} took {amount} damage", who(*player))
        }
        E::LifeTaken { player, card, banished } => {
            let what = if *banished { "banished" } else { "took" };
            match card.id() {
                Some(_) => format!("{} {what} {}", who(*player), name(*card)),
                None => format!("{} {what} a life card", who(*player)),
            }
        }
        E::EffectActivated { source, controller } => {
            format!("{} used {}", who(*controller), name(*source))
        }
        E::NoLegalTargets { source, controller } => {
            let _ = controller;
            format!("{} had no legal target", name(*source))
        }
        E::TriggerActivated { player, card } => {
            format!("{} activated {}'s [Trigger]", who(*player), name(*card))
        }
        E::GameEnded { result } => match result.winner() {
            Some(w) if w == viewer => "You win".into(),
            Some(_) => "You lose".into(),
            None => "Draw".into(),
        },
        // Phase changes, DON!! placement and resting are noise; the board
        // already shows their result.
        _ => return None,
    })
}

pub fn question(pending: &Pending) -> String {
    match pending {
        Pending::Mulligan { .. } => "Keep this hand?".into(),
        Pending::MainAction { .. } => "Your main phase".into(),
        Pending::Block { .. } => "You are being attacked — block?".into(),
        Pending::Counter { .. } => "Counter step".into(),
        Pending::Trigger { .. } => "That life card has a [Trigger]".into(),
        Pending::Choose { up_to, .. } => format!("Choose up to {up_to}"),
    }
}

pub fn action_label(action: &Action, game: &Game) -> String {
    let db = game.db();
    let name = |id: op_core::CardInstanceId| db.get(game.state.card(id).def).name.clone();
    let power = |id: op_core::CardInstanceId| game.derived().power(id);

    match action {
        Action::Mulligan(true) => "Mulligan".into(),
        Action::Mulligan(false) => "Keep".into(),
        Action::EndMainPhase => "End turn".into(),
        Action::PlayCard { card } => format!(
            "Play {} ({})",
            name(*card),
            db.get(game.state.card(*card).def).cost
        ),
        Action::ActivateEffect { card, slot } => {
            // Flagged rather than hidden: activating with no target is legal
            // and still costs, so the player should see the trade rather than
            // discover it afterwards.
            let suffix = if game.activation_finds_targets(*card, *slot) {
                ""
            } else {
                " — no legal target"
            };
            format!("Activate {}{suffix}", name(*card))
        }
        Action::GiveDon { to } => format!("DON!! → {}", name(*to)),
        Action::Attack { attacker, target } => format!(
            "{} ({}) → {} ({})",
            name(*attacker),
            power(*attacker),
            name(*target),
            power(*target)
        ),
        Action::Block { blocker: None } => "Don't block".into(),
        Action::Block {
            blocker: Some(card),
        } => format!("Block with {} ({})", name(*card), power(*card)),
        Action::Counter { card, to } => format!(
            "Counter +{} to {} (trash {})",
            db.get(game.state.card(*card).def).counter.unwrap_or(0),
            name(*to),
            name(*card)
        ),
        Action::CounterEvent { card, to } => {
            format!("Play {} on {}", name(*card), name(*to))
        }
        Action::DoneCountering => "Done".into(),
        Action::UseTrigger(true) => "Activate [Trigger]".into(),
        Action::UseTrigger(false) => "Take into hand".into(),
        Action::Choose { cards } if cards.is_empty() => "Choose nothing".into(),
        Action::Choose { cards } => {
            let names: Vec<String> = cards.iter().map(|&c| name(c)).collect();
            names.join(", ")
        }
    }
}
