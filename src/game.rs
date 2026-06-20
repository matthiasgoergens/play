//! Core rock-paper-scissors rules and match state.
//!
//! This module is pure and provider-agnostic: it knows nothing about LLMs or
//! the network. Everything an agent needs to make a decision is exposed through
//! [`PlayerView`], and everything that happened is recorded in [`MatchState`].

use std::fmt;

/// One of the three throws.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Move {
    Rock,
    Paper,
    Scissors,
}

impl Move {
    pub const ALL: [Move; 3] = [Move::Rock, Move::Paper, Move::Scissors];

    /// True if `self` beats `other` under standard RPS rules.
    pub fn beats(self, other: Move) -> bool {
        matches!(
            (self, other),
            (Move::Rock, Move::Scissors)
                | (Move::Paper, Move::Rock)
                | (Move::Scissors, Move::Paper)
        )
    }

    /// The move that beats `self` (what an opponent should play to win).
    pub fn loses_to(self) -> Move {
        match self {
            Move::Rock => Move::Paper,
            Move::Paper => Move::Scissors,
            Move::Scissors => Move::Rock,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Move::Rock => "rock",
            Move::Paper => "paper",
            Move::Scissors => "scissors",
        }
    }

    /// Lenient parse: finds the first move keyword appearing in `s`, so it
    /// tolerates models that wrap the answer in prose ("I'll play Rock.").
    pub fn parse(s: &str) -> Option<Move> {
        let lower = s.to_lowercase();
        // Scan left-to-right and take whichever keyword appears earliest.
        let mut best: Option<(usize, Move)> = None;
        for (kw, mv) in [
            ("rock", Move::Rock),
            ("paper", Move::Paper),
            ("scissors", Move::Scissors),
        ] {
            if let Some(idx) = lower.find(kw) {
                if best.map_or(true, |(b, _)| idx < b) {
                    best = Some((idx, mv));
                }
            }
        }
        best.map(|(_, mv)| mv)
    }
}

impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of a round from one seat's perspective.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Win,
    Loss,
    Draw,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Win => "win",
            Outcome::Loss => "loss",
            Outcome::Draw => "draw",
        }
    }
}

/// A completed round: the simultaneous throws of seat 0 and seat 1.
#[derive(Clone, Copy, Debug)]
pub struct Round {
    pub a: Move,
    pub b: Move,
}

impl Round {
    /// Outcome for seat 0 (`a`). Seat 1's outcome is the mirror.
    pub fn outcome_for_a(&self) -> Outcome {
        if self.a == self.b {
            Outcome::Draw
        } else if self.a.beats(self.b) {
            Outcome::Win
        } else {
            Outcome::Loss
        }
    }
}

/// The full record of a match. Seat 0 and seat 1 are symmetric.
#[derive(Clone, Debug)]
pub struct MatchState {
    pub name_a: String,
    pub name_b: String,
    pub rounds: Vec<Round>,
    pub total_rounds: usize,
}

impl MatchState {
    pub fn new(name_a: impl Into<String>, name_b: impl Into<String>, total_rounds: usize) -> Self {
        MatchState {
            name_a: name_a.into(),
            name_b: name_b.into(),
            rounds: Vec::new(),
            total_rounds,
        }
    }

    /// (wins_a, wins_b, draws)
    pub fn score(&self) -> (usize, usize, usize) {
        let mut a = 0;
        let mut b = 0;
        let mut d = 0;
        for r in &self.rounds {
            match r.outcome_for_a() {
                Outcome::Win => a += 1,
                Outcome::Loss => b += 1,
                Outcome::Draw => d += 1,
            }
        }
        (a, b, d)
    }

    /// Build the perspective-specific view handed to the player in `seat`.
    pub fn view_for(&self, seat: Seat) -> PlayerView {
        let history = self
            .rounds
            .iter()
            .map(|r| {
                let (mine, theirs) = match seat {
                    Seat::A => (r.a, r.b),
                    Seat::B => (r.b, r.a),
                };
                let outcome = match (seat, r.outcome_for_a()) {
                    (Seat::A, o) => o,
                    (Seat::B, Outcome::Win) => Outcome::Loss,
                    (Seat::B, Outcome::Loss) => Outcome::Win,
                    (Seat::B, Outcome::Draw) => Outcome::Draw,
                };
                PastRound { mine, theirs, outcome }
            })
            .collect();

        let (my_name, opp_name) = match seat {
            Seat::A => (self.name_a.clone(), self.name_b.clone()),
            Seat::B => (self.name_b.clone(), self.name_a.clone()),
        };

        PlayerView {
            my_name,
            opponent_name: opp_name,
            round_number: self.rounds.len() + 1,
            total_rounds: self.total_rounds,
            history,
        }
    }
}

/// Which side of the table a player sits on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Seat {
    A,
    B,
}

/// A past round as seen by one player.
#[derive(Clone, Copy, Debug)]
pub struct PastRound {
    pub mine: Move,
    pub theirs: Move,
    pub outcome: Outcome,
}

/// Everything a player is told before choosing its next move. The opponent's
/// move for the *current* round is deliberately absent — throws are simultaneous.
#[derive(Clone, Debug)]
pub struct PlayerView {
    pub my_name: String,
    pub opponent_name: String,
    pub round_number: usize,
    pub total_rounds: usize,
    pub history: Vec<PastRound>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rock_beats_scissors_only() {
        assert!(Move::Rock.beats(Move::Scissors));
        assert!(!Move::Rock.beats(Move::Paper));
        assert!(!Move::Rock.beats(Move::Rock));
    }

    #[test]
    fn loses_to_is_the_counter() {
        for m in Move::ALL {
            assert!(m.loses_to().beats(m));
        }
    }

    #[test]
    fn parse_is_lenient_and_ordered() {
        assert_eq!(Move::parse("ROCK"), Some(Move::Rock));
        assert_eq!(Move::parse("I will play scissors this time"), Some(Move::Scissors));
        // Earliest keyword wins when several appear.
        assert_eq!(Move::parse("not paper, rock"), Some(Move::Paper));
        assert_eq!(Move::parse("lizard"), None);
    }

    #[test]
    fn scoring_counts_each_seat() {
        let mut m = MatchState::new("A", "B", 3);
        m.rounds.push(Round { a: Move::Rock, b: Move::Scissors }); // A win
        m.rounds.push(Round { a: Move::Rock, b: Move::Paper }); // B win
        m.rounds.push(Round { a: Move::Rock, b: Move::Rock }); // draw
        assert_eq!(m.score(), (1, 1, 1));
    }

    #[test]
    fn view_mirrors_for_seat_b() {
        let mut m = MatchState::new("A", "B", 1);
        m.rounds.push(Round { a: Move::Rock, b: Move::Scissors });
        let va = m.view_for(Seat::A);
        let vb = m.view_for(Seat::B);
        assert_eq!(va.history[0].outcome, Outcome::Win);
        assert_eq!(vb.history[0].outcome, Outcome::Loss);
        assert_eq!(vb.history[0].mine, Move::Scissors);
    }
}
