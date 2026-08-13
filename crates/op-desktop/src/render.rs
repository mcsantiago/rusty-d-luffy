//! Turning engine types into strings for the UI.
//!
//! Event rendering takes a [`PlayerEvent`], so a card the viewer may not
//! identify has no id to look up and cannot be named by accident.

use op_core::{Action, CardRef, DonClass, Game, Pending, PlayerEvent, PlayerId};

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
        E::LifeTaken {
            player,
            card,
            banished,
        } => {
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
        // Worth a line where routine DON!! placement is not: this DON!! is gone
        // from the field, and the cost area alone does not say where to.
        E::DonSpentToDonDeck { player, count } => {
            format!(
                "{} returned {count} DON!! to their DON!! deck",
                who(*player)
            )
        }
        // Phase changes, DON!! placement and resting are noise; the board
        // already shows their result.
        _ => return None,
    })
}

/// Names an activation cost in the player's terms, so "Pay … to activate?" is
/// answerable without knowing the card's text by heart.
pub fn cost_label(cost: &op_core::script::ActivationCost) -> String {
    let mut parts: Vec<String> = Vec::new();
    if cost.don_minus > 0 {
        parts.push(format!("DON!! -{}", cost.don_minus));
    }
    if cost.rest_don > 0 {
        parts.push(format!("rest {} DON!!", cost.rest_don));
    }
    if cost.rest_self {
        parts.push("rest this card".into());
    }
    if cost.trash_from_hand > 0 {
        parts.push(format!("trash {} from hand", cost.trash_from_hand));
    }
    if cost.life_to_hand > 0 {
        parts.push(format!("take {} Life card(s) into hand", cost.life_to_hand));
    }
    if parts.is_empty() {
        "nothing".into()
    } else {
        parts.join(" and ")
    }
}

/// Why an activation would currently achieve nothing, in a sentence.
///
/// Naming the empty pool is the whole value. That the ability will do nothing a
/// player can work out; *which* of the things it needs is missing — and so what
/// they would have to change to make it worth activating — they cannot.
pub fn shortfall_label(req: &op_core::Requirement<'_>) -> String {
    use op_core::effect::DonSource;
    use op_core::Requirement;

    match req {
        Requirement::Cards(select) => format!("No {} to choose.", wants_label(select)),
        Requirement::Condition => "This card's condition is not met.".into(),
        Requirement::Don(DonSource::Rested) => "No rested DON!! in your cost area.".into(),
        Requirement::Don(DonSource::Active) => "No active DON!! in your cost area.".into(),
        Requirement::Don(DonSource::Any) => "No DON!! in your cost area.".into(),
    }
}

/// What a `choose` is asking for, as a noun phrase: "rested DON!! in your cost
/// area", "your Characters".
///
/// Deliberately loose. It describes the zone, the owner and the filters a
/// player can act on, and stays silent about the ones that would turn a warning
/// into a rules lecture. The printed card text is on screen beside it.
fn wants_label(select: &op_core::effect::Selector) -> String {
    use op_core::effect::{Filter, Who};
    use op_core::zone::Zone;

    let mut what = String::new();
    for filter in &select.filters {
        match filter {
            Filter::IsRested(true) => what.push_str("rested "),
            Filter::IsRested(false) => what.push_str("active "),
            Filter::HasKeyword(k) => what.push_str(&format!("[{k:?}] ")),
            _ => {}
        }
    }

    what.push_str(match select.zone {
        Zone::Cost | Zone::DonDeck => "DON!!",
        Zone::Character => "Characters",
        Zone::Hand => "cards in hand",
        Zone::Trash => "cards in the trash",
        // The selector's marker for the battlers — Leader and Characters
        // together — not the Leader alone. `your_battlers` is written this way.
        Zone::Leader => "Leader or Characters",
        Zone::Stage => "Stage",
        Zone::Deck => "cards in the deck",
        Zone::Life => "Life cards",
        Zone::Limbo => "cards",
    });

    let whose = match (select.owner, select.zone) {
        (Who::You, Zone::Cost) => " in your cost area",
        (Who::You, _) => " of yours",
        (Who::Opponent, Zone::Cost) => " in your opponent's cost area",
        (Who::Opponent, _) => " of your opponent's",
        (Who::Both, _) => "",
    };
    what.push_str(whose);
    what
}

pub fn question(pending: &Pending) -> String {
    match pending {
        Pending::Mulligan { .. } => "Keep this hand?".into(),
        Pending::MainAction { .. } => "Your main phase".into(),
        Pending::Block { .. } => "You are being attacked — block?".into(),
        Pending::Counter { .. } => "Counter step".into(),
        Pending::Trigger { .. } => "That life card has a [Trigger]".into(),
        Pending::PayCost { cost, .. } => format!("Pay {} to activate?", cost_label(cost)),
        // The count is fixed by the cost (8-3-1-6); only which ones is open.
        Pending::ReturnDon { n, .. } => format!("Return {n} DON!! to your DON!! deck"),
        Pending::Arrange { cards, .. } => format!(
            "Put {} back on your deck — top or bottom, in any order",
            match cards.len() {
                1 => "this card".to_string(),
                n => format!("these {n} cards"),
            }
        ),
        // "up to" is an offer the player may decline; a non-zero floor is an
        // instruction, and saying "up to" there would suggest otherwise.
        Pending::Choose {
            up_to, at_least, ..
        } => {
            if *at_least == 0 {
                format!("Choose up to {up_to}")
            } else if at_least == up_to {
                format!("Choose {up_to}")
            } else {
                format!("Choose {at_least} to {up_to}")
            }
        }
    }
}

/// Names one card a decision may take, for the choice grid.
///
/// DON!! are the interesting case: they are interchangeable, have no art (#29),
/// and a given one is not in the cost area the view publishes — so where it
/// sits is both the only thing that distinguishes it and the only thing the
/// client cannot work out for itself.
pub fn candidate_label(id: op_core::CardInstanceId, game: &Game) -> String {
    let def = game.db().get(game.state.card(id).def);
    if def.category != op_core::card::Category::Don {
        return def.name.clone();
    }
    match game.don_class(id) {
        DonClass::Given(holder) => format!(
            "DON!! on {}",
            game.db().get(game.state.card(holder).def).name
        ),
        DonClass::Active => "DON!! (active)".into(),
        DonClass::Rested => "DON!! (rested)".into(),
    }
}

/// Counts each class once — "2 rested, 1 on Zoro" — because naming ten
/// interchangeable DON!! one at a time is the same phrase ten times over.
fn don_tally(
    dons: &[op_core::CardInstanceId],
    game: &Game,
    whence: impl Fn(DonClass) -> String,
) -> String {
    let mut tally: Vec<(DonClass, usize)> = Vec::new();
    for &d in dons {
        let class = game.don_class(d);
        match tally.iter_mut().find(|(c, _)| *c == class) {
            Some((_, n)) => *n += 1,
            None => tally.push((class, 1)),
        }
    }
    tally
        .into_iter()
        .map(|(class, n)| format!("{n} {}", whence(class)))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn action_label(action: &Action, game: &Game) -> String {
    let db = game.db();
    let name = |id: op_core::CardInstanceId| db.get(game.state.card(id).def).name.clone();
    let power = |id: op_core::CardInstanceId| game.derived().power(id);

    match action {
        Action::Mulligan(true) => "Mulligan".into(),
        Action::Mulligan(false) => "Keep".into(),
        Action::PayCost(true) => "Pay the cost".into(),
        Action::PayCost(false) => "Don't pay".into(),
        Action::Arrange { top, bottom } => {
            // Every option here is the same cards in a different arrangement,
            // so the label has to spell the arrangement out or they all read
            // alike.
            let list = |ids: &[op_core::CardInstanceId]| {
                if ids.is_empty() {
                    "—".to_string()
                } else {
                    ids.iter()
                        .map(|&id| name(id))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            };
            format!("Top: {} · Bottom: {}", list(top), list(bottom))
        }
        Action::EndMainPhase => "End turn".into(),
        Action::PlayCard { card, replacing } => {
            let suffix = if game.play_finds_targets(*card) {
                ""
            } else {
                " — no target"
            };
            let room = match replacing {
                Some(victim) => format!(" — trashing {}", name(*victim)),
                None => String::new(),
            };
            format!(
                "Play {} ({}){room}{suffix}",
                name(*card),
                db.get(game.state.card(*card).def).cost
            )
        }
        Action::ActivateEffect {
            card,
            slot,
            discard,
        } => {
            // Flagged rather than hidden: activating with no target is legal
            // and still costs, so the player should see the trade rather than
            // discover it afterwards.
            let suffix = if game.activation_finds_targets(*card, *slot) {
                ""
            } else {
                " — no target"
            };
            let cost = match discard.as_slice() {
                [] => String::new(),
                cards => format!(
                    " — trashing {}",
                    cards
                        .iter()
                        .map(|&c| name(c))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            format!("Activate {}{cost}{suffix}", name(*card))
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
        // DON!! cards are interchangeable, so the option is named by where they
        // sit — which is the entire substance of the choice.
        Action::ReturnDon { dons } => don_tally(dons, game, |class| match class {
            DonClass::Given(holder) => format!("on {}", name(holder)),
            DonClass::Active => "active".into(),
            DonClass::Rested => "rested".into(),
        }),
    }
}
