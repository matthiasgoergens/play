//! Command-line referee. Pits two players against each other over N rounds and
//! prints the play-by-play and final score. Each game is also saved to disk.

use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use rps::game::Outcome;
use rps::referee::{play_match, RoundReport};
use rps::transcript::Recorder;

/// Iterated rock-paper-scissors between two agents.
#[derive(Parser, Debug)]
#[command(name = "rps", about = "Play iterated rock-paper-scissors between agents")]
struct Args {
    /// First player spec: random | counter | fixed:rock | anthropic[:model] | gemini[:model]
    #[arg(long, default_value = "anthropic")]
    p1: String,

    /// Second player spec (same grammar as --p1).
    #[arg(long, default_value = "gemini")]
    p2: String,

    /// Number of rounds to play.
    #[arg(long, default_value_t = 10)]
    rounds: usize,

    /// Seed for the local random/counter players (for reproducible matches).
    #[arg(long)]
    seed: Option<u64>,

    /// Base directory for saved game transcripts.
    #[arg(long, default_value = "games")]
    games_dir: String,

    /// Don't save a transcript for this match.
    #[arg(long)]
    no_record: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let p1 = rps::build_player(&args.p1, args.seed)?;
    let p2 = rps::build_player(&args.p2, args.seed)?;

    println!("Match: {} vs {}  ({} rounds)\n", p1.name(), p2.name(), args.rounds);

    let name_a = p1.name().to_string();
    let name_b = p2.name().to_string();

    let recorder = if args.no_record {
        None
    } else {
        let r = Recorder::create(Path::new(&args.games_dir), &name_a, &name_b)?;
        println!("recording to {}\n", r.dir().display());
        Some(Arc::new(r))
    };

    let on_round = move |r: &RoundReport| {
        let verdict = match r.outcome_a {
            Outcome::Win => format!("{name_a} wins"),
            Outcome::Loss => format!("{name_b} wins"),
            Outcome::Draw => "draw".to_string(),
        };
        println!(
            "Round {:>2}: {} = {:<8}  {} = {:<8}  -> {}",
            r.number, name_a, r.move_a, name_b, r.move_b, verdict
        );
        if let Some(note) = &r.note_a {
            println!("            {name_a}: {note}");
        }
        if let Some(note) = &r.note_b {
            println!("            {name_b}: {note}");
        }
    };

    let result =
        play_match(p1.as_ref(), p2.as_ref(), args.rounds, recorder.as_deref(), on_round).await;

    let state = match result {
        Ok(state) => {
            if let Some(rec) = &recorder {
                let _ = rec.finish(None);
            }
            state
        }
        Err(e) => {
            if let Some(rec) = &recorder {
                let _ = rec.finish(Some(&format!("{e:#}")));
                println!("\nsaved partial game to {}", rec.dir().display());
            }
            return Err(e);
        }
    };

    let (a, b, d) = state.score();
    println!("\nFinal score: {} {a} — {b} {} (draws: {d})", state.name_a, state.name_b);
    let winner = if a > b {
        format!("{} wins the match!", state.name_a)
    } else if b > a {
        format!("{} wins the match!", state.name_b)
    } else {
        "The match is a tie.".to_string()
    };
    println!("{winner}");
    if let Some(rec) = &recorder {
        println!("saved to {}", rec.dir().display());
    }

    Ok(())
}
