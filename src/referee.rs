//! The referee: drives an iterated match, asking both players for a move each
//! round, recording the result, and emitting events for display.

use anyhow::Context;

use crate::game::{MatchState, Move, Outcome, Round, Seat};
use crate::player::Player;

/// What happened in one round, ready to print or stream to a UI.
#[derive(Clone, Debug)]
pub struct RoundReport {
    pub number: usize,
    pub move_a: Move,
    pub move_b: Move,
    pub note_a: Option<String>,
    pub note_b: Option<String>,
    pub outcome_a: Outcome,
}

/// Run `total_rounds` of simultaneous-reveal RPS between two players, invoking
/// `on_round` after each round (e.g. to print it). Both players are queried
/// concurrently from the same shared history.
pub async fn play_match(
    player_a: &dyn Player,
    player_b: &dyn Player,
    total_rounds: usize,
    mut on_round: impl FnMut(&RoundReport),
) -> anyhow::Result<MatchState> {
    let mut state = MatchState::new(player_a.name(), player_b.name(), total_rounds);

    for n in 1..=total_rounds {
        let view_a = state.view_for(Seat::A);
        let view_b = state.view_for(Seat::B);

        // Throws are simultaneous: query both before either result is known.
        // If either player can't produce a move, the match stops here; rounds
        // already played remain recorded and displayed.
        let (da, db) = tokio::join!(player_a.decide(&view_a), player_b.decide(&view_b));
        let da = da.with_context(|| {
            format!("{} couldn't make a move in round {n} — match stopped", player_a.name())
        })?;
        let db = db.with_context(|| {
            format!("{} couldn't make a move in round {n} — match stopped", player_b.name())
        })?;

        let round = Round { a: da.mv, b: db.mv };
        let report = RoundReport {
            number: n,
            move_a: da.mv,
            move_b: db.mv,
            note_a: da.note,
            note_b: db.note,
            outcome_a: round.outcome_for_a(),
        };
        state.rounds.push(round);
        on_round(&report);
    }

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::{CounterPlayer, FixedPlayer};

    #[tokio::test]
    async fn match_runs_requested_rounds_and_counter_wins() {
        let a = CounterPlayer::new(Some(7));
        let b = FixedPlayer::new(Move::Rock);
        let state = play_match(&a, &b, 12, |_| {}).await.unwrap();
        assert_eq!(state.rounds.len(), 12);
        let (wa, wb, _) = state.score();
        assert!(wa > wb);
    }
}
