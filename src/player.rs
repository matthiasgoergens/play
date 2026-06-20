//! The `Player` abstraction every contestant implements, plus a few local
//! (non-network) players used for testing and as opponents.
//!
//! An LLM-backed player is just another `Player` (see [`crate::agents`]). This
//! is the seam an OAuth-connected retail agent would plug into later.

use async_trait::async_trait;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use std::sync::Mutex;

use crate::game::{Move, PlayerView};

/// A move plus an optional note (trash talk / reasoning) for display.
#[derive(Clone, Debug)]
pub struct Decision {
    /// The chosen move, or `None` if the player produced output we couldn't
    /// parse a move from. The raw output is still carried in `note` so it can
    /// be saved before the move is resolved ("write first, parse later").
    pub mv: Option<Move>,
    pub note: Option<String>,
}

impl Decision {
    /// A definite move with no commentary (local players).
    pub fn new(mv: Move) -> Self {
        Decision { mv: Some(mv), note: None }
    }
}

#[async_trait]
pub trait Player: Send + Sync {
    fn name(&self) -> &str;

    /// Choose a move given the shared history so far. Throws are simultaneous,
    /// so the view never contains the opponent's current-round move.
    async fn decide(&self, view: &PlayerView) -> anyhow::Result<Decision>;
}

/// Always plays a fixed move. Useful as a punching bag in tests.
pub struct FixedPlayer {
    name: String,
    mv: Move,
}

impl FixedPlayer {
    pub fn new(mv: Move) -> Self {
        FixedPlayer {
            name: format!("fixed:{mv}"),
            mv,
        }
    }
}

#[async_trait]
impl Player for FixedPlayer {
    fn name(&self) -> &str {
        &self.name
    }
    async fn decide(&self, _view: &PlayerView) -> anyhow::Result<Decision> {
        Ok(Decision::new(self.mv))
    }
}

/// Uniformly random. The theoretically unexploitable baseline.
pub struct RandomPlayer {
    name: String,
    rng: Mutex<rand::rngs::StdRng>,
}

impl RandomPlayer {
    pub fn new(seed: Option<u64>) -> Self {
        let rng = match seed {
            Some(s) => rand::rngs::StdRng::seed_from_u64(s),
            None => rand::rngs::StdRng::from_entropy(),
        };
        RandomPlayer {
            name: "random".to_string(),
            rng: Mutex::new(rng),
        }
    }
}

#[async_trait]
impl Player for RandomPlayer {
    fn name(&self) -> &str {
        &self.name
    }
    async fn decide(&self, _view: &PlayerView) -> anyhow::Result<Decision> {
        let mv = *Move::ALL.choose(&mut *self.rng.lock().unwrap()).unwrap();
        Ok(Decision::new(mv))
    }
}

/// Plays the move that would have beaten the opponent's most frequent throw so
/// far (random on the first round). A simple exploitative strategy — handy for
/// a deterministic end-to-end test against `FixedPlayer`.
pub struct CounterPlayer {
    name: String,
    rng: Mutex<rand::rngs::StdRng>,
}

impl CounterPlayer {
    pub fn new(seed: Option<u64>) -> Self {
        let rng = match seed {
            Some(s) => rand::rngs::StdRng::seed_from_u64(s),
            None => rand::rngs::StdRng::from_entropy(),
        };
        CounterPlayer {
            name: "counter".to_string(),
            rng: Mutex::new(rng),
        }
    }
}

#[async_trait]
impl Player for CounterPlayer {
    fn name(&self) -> &str {
        &self.name
    }
    async fn decide(&self, view: &PlayerView) -> anyhow::Result<Decision> {
        let mut counts = [0usize; 3];
        for past in &view.history {
            let idx = match past.theirs {
                Move::Rock => 0,
                Move::Paper => 1,
                Move::Scissors => 2,
            };
            counts[idx] += 1;
        }
        if view.history.is_empty() {
            let mv = *Move::ALL.choose(&mut *self.rng.lock().unwrap()).unwrap();
            return Ok(Decision::new(mv));
        }
        let predicted = Move::ALL[counts
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| **c)
            .map(|(i, _)| i)
            .unwrap()];
        Ok(Decision {
            mv: Some(predicted.loses_to()),
            note: Some(format!("countering your frequent {predicted}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Seat;
    use crate::game::MatchState;

    #[tokio::test]
    async fn counter_eventually_beats_a_fixed_player() {
        // Seat A is a CounterPlayer; seat B always throws Rock.
        let counter = CounterPlayer::new(Some(1));
        let mut state = MatchState::new("counter", "fixed:rock", 10);
        for _ in 0..10 {
            let d = counter.decide(&state.view_for(Seat::A)).await.unwrap();
            state.rounds.push(crate::game::Round { a: d.mv.unwrap(), b: Move::Rock });
        }
        let (a, b, _d) = state.score();
        // After it has seen Rock a few times it should be playing Paper and winning.
        assert!(a > b, "counter ({a}) should out-score fixed rock ({b})");
    }

    #[tokio::test]
    async fn random_is_seeded_and_deterministic() {
        let p = RandomPlayer::new(Some(42));
        let state = MatchState::new("random", "x", 1);
        let first = p.decide(&state.view_for(Seat::A)).await.unwrap().mv;
        let p2 = RandomPlayer::new(Some(42));
        let again = p2.decide(&state.view_for(Seat::A)).await.unwrap().mv;
        assert_eq!(first, again);
    }
}
