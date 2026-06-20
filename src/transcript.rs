//! Persisting matches for posterity.
//!
//! Each game gets its own directory under a base (default `games/`). Every move
//! is one text file whose name encodes *when* (round) and *who* (seat +
//! player); the file body is that model's full reply for the round. The move
//! itself is deliberately *not* in the filename — it may not parse — so the
//! moves live only in `summary.txt`, alongside the final score. Files are
//! written as each round completes, so a match that stops early still leaves a
//! complete record of what was played.
//!
//! Example layout:
//! ```text
//! games/1718900000_anthropic-claude-opus-4-8_vs_gemini-2.5-flash/
//!   r001_A_anthropic-claude-opus-4-8.txt        # body = that model's round-1 reply
//!   r001_B_gemini-2.5-flash.txt
//!   r002_A_anthropic-claude-opus-4-8.txt
//!   ...
//!   summary.txt                                 # move table + final score
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::game::{Move, Outcome};
use crate::referee::RoundReport;

/// Keep filenames filesystem-safe and readable.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' })
        .collect()
}

struct Played {
    a: Move,
    b: Move,
    outcome_a: Outcome,
}

pub struct Recorder {
    dir: PathBuf,
    label_a: String,
    label_b: String,
    played: Mutex<Vec<Played>>,
}

impl Recorder {
    /// Create `<base>/<timestamp>_<a>_vs_<b>/` and return a recorder writing to it.
    pub fn create(base: &Path, label_a: &str, label_b: &str) -> io::Result<Recorder> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dir = base.join(format!(
            "{ts}_{}_vs_{}",
            sanitize(label_a),
            sanitize(label_b)
        ));
        fs::create_dir_all(&dir)?;
        Ok(Recorder {
            dir,
            label_a: label_a.to_string(),
            label_b: label_b.to_string(),
            played: Mutex::new(Vec::new()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Write both players' replies for one completed round.
    pub fn record(&self, r: &RoundReport) -> io::Result<()> {
        self.write_reply("A", &self.label_a, r.number, r.note_a.as_deref())?;
        self.write_reply("B", &self.label_b, r.number, r.note_b.as_deref())?;
        self.played.lock().unwrap().push(Played {
            a: r.move_a,
            b: r.move_b,
            outcome_a: r.outcome_a,
        });
        Ok(())
    }

    fn write_reply(
        &self,
        seat: &str,
        label: &str,
        round: usize,
        note: Option<&str>,
    ) -> io::Result<()> {
        let name = format!("r{round:03}_{seat}_{}.txt", sanitize(label));
        let body = note.unwrap_or("(no commentary)");
        fs::write(self.dir.join(name), format!("{body}\n"))
    }

    /// Write `summary.txt`. `stopped` is `Some(reason)` if the match ended early.
    pub fn finish(&self, stopped: Option<&str>) -> io::Result<()> {
        let played = self.played.lock().unwrap();
        let mut a_wins = 0;
        let mut b_wins = 0;
        let mut draws = 0;

        let mut out = String::new();
        out.push_str(&format!("seat A: {}\nseat B: {}\n", self.label_a, self.label_b));
        out.push_str(&format!("rounds played: {}\n\n", played.len()));
        for (i, p) in played.iter().enumerate() {
            let verdict = match p.outcome_a {
                Outcome::Win => {
                    a_wins += 1;
                    "A"
                }
                Outcome::Loss => {
                    b_wins += 1;
                    "B"
                }
                Outcome::Draw => {
                    draws += 1;
                    "draw"
                }
            };
            out.push_str(&format!(
                "round {:03}: A={:<8} B={:<8} -> {}\n",
                i + 1,
                p.a.as_str(),
                p.b.as_str(),
                verdict
            ));
        }
        out.push_str(&format!("\nscore: A {a_wins} — B {b_wins} (draws {draws})\n"));
        if let Some(reason) = stopped {
            out.push_str(&format!("\nstopped early: {reason}\n"));
        }
        fs::write(self.dir.join("summary.txt"), out)
    }
}
