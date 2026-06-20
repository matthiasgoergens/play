//! Iterated rock-paper-scissors between pluggable agents.
//!
//! - [`game`]: pure rules and match state.
//! - [`player`]: the `Player` trait + local (non-network) players.
//! - [`agents`]: LLM-backed players (Anthropic, Gemini) over raw HTTP.
//! - [`referee`]: drives a match and reports each round.
//!
//! The `Player` trait is the seam: a future website that lets users connect
//! their own agents (via API key today, OAuth if one ever exists) only needs to
//! provide another `Player` implementation.

pub mod agents;
pub mod game;
pub mod player;
pub mod referee;

use anyhow::{anyhow, Context};

/// Build a player from a CLI spec string.
///
/// Examples: `random`, `fixed:rock`, `counter`, `anthropic`,
/// `anthropic:claude-opus-4-8`, `gemini`, `gemini:gemini-2.5-pro`.
pub fn build_player(
    spec: &str,
    seed: Option<u64>,
) -> anyhow::Result<Box<dyn player::Player>> {
    let (kind, arg) = match spec.split_once(':') {
        Some((k, a)) => (k, Some(a.to_string())),
        None => (spec, None),
    };

    Ok(match kind {
        "random" => Box::new(player::RandomPlayer::new(seed)),
        "counter" => Box::new(player::CounterPlayer::new(seed)),
        "fixed" => {
            let mv = arg
                .as_deref()
                .and_then(game::Move::parse)
                .ok_or_else(|| anyhow!("fixed player needs a move, e.g. fixed:rock"))?;
            Box::new(player::FixedPlayer::new(mv))
        }
        "anthropic" => Box::new(
            agents::AnthropicPlayer::from_env(arg).context("building Anthropic player")?,
        ),
        "gemini" => {
            Box::new(agents::GeminiPlayer::from_env(arg).context("building Gemini player")?)
        }
        other => return Err(anyhow!("unknown player kind: {other:?}")),
    })
}
