//! The referee: drives an iterated match, asking both players for a move each
//! round, recording the result, and emitting events for display.
//!
//! Ordering within a round is "write first, parse later": once both players
//! have replied, each raw reply is handed to the recorder *before* we try to
//! resolve a move from it. So if a model says something we can't parse, its
//! words are already saved, and only then does the match stop.

use anyhow::{anyhow, Context};

use crate::game::{MatchState, Move, Outcome, Round, Seat};
use crate::player::Player;
use crate::transcript::Recorder;

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
/// `on_round` after each completed round (e.g. to print it). Both players are
/// queried concurrently from the same shared history. If `recorder` is set,
/// every reply is saved as it arrives — including the one that stops the match.
pub async fn play_match(
    player_a: &dyn Player,
    player_b: &dyn Player,
    total_rounds: usize,
    recorder: Option<&Recorder>,
    mut on_round: impl FnMut(&RoundReport),
) -> anyhow::Result<MatchState> {
    let mut state = MatchState::new(player_a.name(), player_b.name(), total_rounds);

    for n in 1..=total_rounds {
        let view_a = state.view_for(Seat::A);
        let view_b = state.view_for(Seat::B);

        // Throws are simultaneous: query both before either result is known.
        // A transport failure (not a parse failure) propagates here.
        let (da, db) = tokio::join!(player_a.decide(&view_a), player_b.decide(&view_b));
        let da = da.with_context(|| {
            format!("{} failed to reply in round {n} — match stopped", player_a.name())
        })?;
        let db = db.with_context(|| {
            format!("{} failed to reply in round {n} — match stopped", player_b.name())
        })?;

        // Write first: persist each raw reply before attempting to parse a move.
        if let Some(rec) = recorder {
            let _ = rec.reply(n, "A", player_a.name(), da.note.as_deref());
            let _ = rec.reply(n, "B", player_b.name(), db.note.as_deref());
        }

        // Parse later: both sides must have produced a recognizable move.
        let (ma, mb) = match (da.mv, db.mv) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                let mut who = Vec::new();
                if da.mv.is_none() {
                    who.push(player_a.name());
                }
                if db.mv.is_none() {
                    who.push(player_b.name());
                }
                return Err(anyhow!(
                    "{} gave no recognizable move in round {n} (reply saved) — match stopped",
                    who.join(" and ")
                ));
            }
        };

        let round = Round { a: ma, b: mb };
        let outcome_a = round.outcome_for_a();
        if let Some(rec) = recorder {
            rec.round_played(ma, mb, outcome_a);
        }

        let report = RoundReport {
            number: n,
            move_a: ma,
            move_b: mb,
            note_a: da.note,
            note_b: db.note,
            outcome_a,
        };
        state.rounds.push(round);
        on_round(&report);
    }

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::PlayerView;
    use crate::player::{CounterPlayer, Decision, FixedPlayer};
    use async_trait::async_trait;

    #[tokio::test]
    async fn match_runs_requested_rounds_and_counter_wins() {
        let a = CounterPlayer::new(Some(7));
        let b = FixedPlayer::new(Move::Rock);
        let state = play_match(&a, &b, 12, None, |_| {}).await.unwrap();
        assert_eq!(state.rounds.len(), 12);
        let (wa, wb, _) = state.score();
        assert!(wa > wb);
    }

    /// A player that says something but never names a move.
    struct Babble;
    #[async_trait]
    impl Player for Babble {
        fn name(&self) -> &str {
            "babble"
        }
        async fn decide(&self, _view: &PlayerView) -> anyhow::Result<Decision> {
            Ok(Decision { mv: None, note: Some("I refuse to pick.".to_string()) })
        }
    }

    #[tokio::test]
    async fn unparseable_reply_is_written_before_the_match_stops() {
        let base = std::env::temp_dir().join(format!("rps_wf_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let a = Babble;
        let b = FixedPlayer::new(Move::Rock);
        let rec = Recorder::create(&base, a.name(), b.name()).unwrap();

        let result = play_match(&a, &b, 3, Some(&rec), |_| {}).await;

        // The match stops...
        assert!(result.is_err());
        // ...but the un-parseable reply was saved first.
        let saved = std::fs::read_to_string(rec.dir().join("r001_A_babble.txt")).unwrap();
        assert!(saved.contains("I refuse to pick"));
        // And the other player's reply for that same round is saved too.
        assert!(rec.dir().join("r001_B_fixed-rock.txt").exists());

        std::fs::remove_dir_all(&base).ok();
    }
}
