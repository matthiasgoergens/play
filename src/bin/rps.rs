//! Command-line referee. Pits two players against each other over N rounds and
//! prints the play-by-play and final score.

use clap::Parser;
use rps::game::Outcome;
use rps::referee::{play_match, RoundReport};

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let p1 = rps::build_player(&args.p1, args.seed)?;
    let p2 = rps::build_player(&args.p2, args.seed)?;

    println!("Match: {} vs {}  ({} rounds)\n", p1.name(), p2.name(), args.rounds);

    let name_a = p1.name().to_string();
    let name_b = p2.name().to_string();

    let print_round = |r: &RoundReport| {
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

    let state = play_match(p1.as_ref(), p2.as_ref(), args.rounds, print_round).await?;

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

    Ok(())
}
