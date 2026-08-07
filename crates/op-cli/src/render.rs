//! Board rendering.
//!
//! Everything drawn here comes from a [`PlayerView`], never from `GameState`,
//! so the interface physically cannot show the human information they are not
//! entitled to.

use op_core::card::CardDb;
use op_core::view::{PlayerSide, PlayerView, VisibleCard};
use op_core::{Action, CardRef, Game, PlayerEvent, PlayerId};

const RULE: &str = "──────────────────────────────────────────────────────────────";

pub fn board(view: &PlayerView, db: &CardDb) -> String {
    let mut out = String::new();
    out.push_str(RULE);
    out.push('\n');
    out.push_str(&format!(
        "turn {}  ·  {}  ·  {:?} phase\n",
        view.turn,
        if view.turn_player == view.viewer {
            "YOUR TURN"
        } else {
            "opponent's turn"
        },
        view.phase
    ));
    out.push_str(RULE);
    out.push('\n');

    out.push_str(&side(&view.opponent, db, "OPPONENT"));
    out.push('\n');
    out.push_str(&side(&view.you, db, "YOU"));

    out.push_str("\nyour hand:\n");
    if view.you.hand.is_empty() {
        out.push_str("  (empty)\n");
    }
    for (i, card) in view.you.hand.iter().enumerate() {
        out.push_str(&format!("  {i:>2}. {}\n", describe(card, db, true)));
    }
    out
}

fn side(side: &PlayerSide, db: &CardDb, label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{label}  life {}  hand {}  deck {}  DON!! {}/{}\n",
        side.life_count,
        side.hand_count,
        side.deck_count,
        side.don_active,
        side.don_active + side.don_rested
    ));
    if let Some(leader) = &side.leader {
        out.push_str(&format!("  leader: {}\n", describe(leader, db, false)));
    }
    if side.characters.is_empty() {
        out.push_str("  board:  (empty)\n");
    } else {
        out.push_str("  board:  ");
        let cards: Vec<String> = side
            .characters
            .iter()
            .map(|c| describe(c, db, false))
            .collect();
        out.push_str(&cards.join("  |  "));
        out.push('\n');
    }
    if let Some(stage) = &side.stage {
        out.push_str(&format!("  stage:  {}\n", describe(stage, db, false)));
    }
    out
}

fn describe(card: &VisibleCard, db: &CardDb, in_hand: bool) -> String {
    let Some(number) = &card.number else {
        return "?????".to_string();
    };
    let Some(def) = db.by_number(number) else {
        return number.clone();
    };
    let def = db.get(def);

    let mut s = format!("{} [{}]", def.name, number);
    if in_hand {
        s.push_str(&format!(" cost {}", def.cost));
        if let Some(power) = def.power {
            s.push_str(&format!(" · {power}"));
        }
        if let Some(counter) = def.counter {
            s.push_str(&format!(" · ctr {counter}"));
        }
        s.push_str(&format!(" · {:?}", def.category));
    } else {
        if let Some(power) = card.power {
            s.push_str(&format!(" {power}"));
        }
        if card.attached_don > 0 {
            s.push_str(&format!(" +{}DON", card.attached_don));
        }
        if card.rested {
            s.push_str(" (rested)");
        }
    }
    s
}

/// A one-line description of an action, for the numbered choice menu.
pub fn action(action: &Action, game: &Game) -> String {
    let db = game.db();
    let name = |id: op_core::CardInstanceId| {
        let def = db.get(game.state.card(id).def);
        format!("{} [{}]", def.name, def.number)
    };
    let power = |id: op_core::CardInstanceId| game.derived().power(id);

    match action {
        Action::Mulligan(true) => "mulligan (shuffle back and redraw 5)".into(),
        Action::Mulligan(false) => "keep this hand".into(),
        Action::EndMainPhase => "end turn".into(),
        Action::PlayCard { card } => {
            let suffix = if game.play_finds_targets(*card) {
                ""
            } else {
                " (no target)"
            };
            format!(
                "play {} (cost {}){suffix}",
                name(*card),
                db.get(game.state.card(*card).def).cost
            )
        }
        Action::ActivateEffect { card, slot } => {
            let suffix = if game.activation_finds_targets(*card, *slot) {
                ""
            } else {
                " (no target)"
            };
            format!("activate {}'s effect #{slot}{suffix}", name(*card))
        }
        Action::GiveDon { to } => format!("give 1 DON!! to {}", name(*to)),
        Action::Attack { attacker, target } => format!(
            "attack: {} ({}) -> {} ({})",
            name(*attacker),
            power(*attacker),
            name(*target),
            power(*target)
        ),
        Action::Block { blocker: None } => "do not block".into(),
        Action::Block {
            blocker: Some(card),
        } => format!("block with {} ({})", name(*card), power(*card)),
        Action::Counter { card, to } => format!(
            "counter: trash {} for +{} to {}",
            name(*card),
            db.get(game.state.card(*card).def).counter.unwrap_or(0),
            name(*to)
        ),
        Action::CounterEvent { card, to } => {
            format!("play {} as a Counter on {}", name(*card), name(*to))
        }
        Action::DoneCountering => "done countering".into(),
        Action::UseTrigger(true) => "activate the [Trigger]".into(),
        Action::UseTrigger(false) => "take the life card into hand".into(),
        Action::Choose { cards } if cards.is_empty() => "choose nothing".into(),
        Action::Choose { cards } => {
            let names: Vec<String> = cards.iter().map(|&c| name(c)).collect();
            format!("choose {}", names.join(", "))
        }
    }
}

/// A human-readable line for an event, or `None` for events not worth showing.
///
/// Takes a [`PlayerEvent`], not a `GameEvent`: a card the viewer may not
/// identify arrives as [`CardRef::Hidden`] and simply has no id to look up, so
/// this cannot print something the player should not see even by mistake.
pub fn event(event: &PlayerEvent, game: &Game, viewer: PlayerId) -> Option<String> {
    use op_core::PlayerEvent as E;
    let db = game.db();
    let name = |card: CardRef| match card.id() {
        Some(id) => db.get(game.state.card(id).def).name.clone(),
        None => "a card".to_string(),
    };
    let who = |p: PlayerId| if p == viewer { "you" } else { "opponent" };

    Some(match event {
        E::TurnStarted { turn, player } => format!("── turn {turn} ({}) ──", who(*player)),
        E::CardPlayed { player, card, .. } => format!("{} played {}", who(*player), name(*card)),
        E::AttackDeclared { attacker, target } => {
            format!("{} attacks {}", name(*attacker), name(*target))
        }
        E::Blocked { blocker, .. } => format!("{} blocks", name(*blocker)),
        E::Countered {
            player,
            target,
            amount,
            ..
        } => format!("{} counters: {} +{amount}", who(*player), name(*target)),
        E::BattleResolved {
            attacker_power,
            target_power,
            attacker_won,
            ..
        } => format!(
            "battle: {attacker_power} vs {target_power} — {}",
            if *attacker_won { "attacker wins" } else { "attack repelled" }
        ),
        E::KnockedOut { card } => format!("{} is K.O.'d", name(*card)),
        E::DamageDealt { player, amount } => {
            format!("{} take{} {amount} damage", who(*player), if *player == viewer { "" } else { "s" })
        }
        E::EffectActivated { source, controller } => {
            format!("{} used {}", who(*controller), name(*source))
        }
        E::NoLegalTargets { source, .. } => {
            format!("{} had no legal target", name(*source))
        }
        E::TriggerActivated { player, card } => {
            format!("{} activates {}'s [Trigger]", who(*player), name(*card))
        }
        E::GameEnded { result } => match result.winner() {
            Some(w) if w == viewer => "*** you win ***".into(),
            Some(_) => "*** you lose ***".into(),
            None => "*** draw ***".into(),
        },
        // Phase changes, draws, DON!! placement and resting are noise at this
        // level of detail; the board readout already shows their result.
        _ => return None,
    })
}
