//! LLM-backed players. Each adapter turns the shared [`PlayerView`] into a
//! prompt, calls a provider over HTTP, and parses a move back out.
//!
//! No official Anthropic/Google SDK exists for Rust, so these talk raw HTTP via
//! `reqwest`. API keys are read from the environment; nothing is hard-coded.

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde_json::json;

use crate::game::{Move, Outcome, PlayerView};
use crate::player::{Decision, Player};

/// Shared instructions + rendered history. Both providers get the same text so
/// matches stay comparable across backends.
fn build_prompt(view: &PlayerView) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "We are playing an iterated game of rock-paper-scissors.\n\
         You are \"{me}\". Your opponent is \"{opp}\".\n\
         This is round {round} of {total}. Moves are revealed simultaneously.\n\n",
        me = view.my_name,
        opp = view.opponent_name,
        round = view.round_number,
        total = view.total_rounds,
    ));

    if view.history.is_empty() {
        s.push_str("No rounds have been played yet.\n\n");
    } else {
        s.push_str("History so far (most recent last):\n");
        for (i, past) in view.history.iter().enumerate() {
            let result = match past.outcome {
                Outcome::Win => "you won",
                Outcome::Loss => "you lost",
                Outcome::Draw => "draw",
            };
            s.push_str(&format!(
                "  round {n}: you played {mine}, opponent played {theirs} -> {result}\n",
                n = i + 1,
                mine = past.mine,
                theirs = past.theirs,
            ));
        }
        let (w, l, d) = tally(view);
        s.push_str(&format!("\nScore so far: you {w}, opponent {l}, draws {d}.\n\n"));
    }

    s.push_str(
        "Choose your move for this round. Reason briefly about the opponent's \
         pattern if it helps, then end your reply with a final line of exactly \
         one word: rock, paper, or scissors.",
    );
    s
}

fn tally(view: &PlayerView) -> (usize, usize, usize) {
    let mut w = 0;
    let mut l = 0;
    let mut d = 0;
    for p in &view.history {
        match p.outcome {
            Outcome::Win => w += 1,
            Outcome::Loss => l += 1,
            Outcome::Draw => d += 1,
        }
    }
    (w, l, d)
}

/// Parse a move from free-form model text, preferring the last keyword (we ask
/// the model to put its decision on the final line).
fn parse_last_move(text: &str) -> anyhow::Result<Move> {
    // Try the last non-empty line first, then fall back to the whole text.
    if let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty()) {
        if let Some(mv) = Move::parse(line) {
            return Ok(mv);
        }
    }
    Move::parse(text).ok_or_else(|| anyhow!("could not find a move in model output: {text:?}"))
}

const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-4-8";
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";

/// Claude via the Messages API (`POST /v1/messages`).
pub struct AnthropicPlayer {
    name: String,
    model: String,
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicPlayer {
    /// Reads `ANTHROPIC_API_KEY` from the environment.
    pub fn from_env(model: Option<String>) -> anyhow::Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY is not set (needed for the Anthropic player)")?;
        let model = model.unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string());
        Ok(AnthropicPlayer {
            name: format!("anthropic:{model}"),
            model,
            api_key,
            client: reqwest::Client::new(),
        })
    }
}

#[async_trait]
impl Player for AnthropicPlayer {
    fn name(&self) -> &str {
        &self.name
    }

    async fn decide(&self, view: &PlayerView) -> anyhow::Result<Decision> {
        let body = json!({
            "model": self.model,
            "max_tokens": 512,
            "system": "You are a competitive rock-paper-scissors player. Play to win.",
            "messages": [{ "role": "user", "content": build_prompt(view) }],
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("request to Anthropic failed")?;

        let status = resp.status();
        let value: serde_json::Value = resp.json().await.context("invalid JSON from Anthropic")?;
        if !status.is_success() {
            return Err(anyhow!("Anthropic returned {status}: {value}"));
        }

        // Concatenate all text blocks in the response content.
        let text: String = value["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        let mv = parse_last_move(&text)?;
        Ok(Decision {
            mv,
            note: first_line(&text),
        })
    }
}

/// Gemini via the Generative Language API (`:generateContent`).
pub struct GeminiPlayer {
    name: String,
    model: String,
    api_key: String,
    client: reqwest::Client,
}

impl GeminiPlayer {
    /// Reads `GEMINI_API_KEY` (or `GOOGLE_API_KEY`) from the environment.
    pub fn from_env(model: Option<String>) -> anyhow::Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .context("GEMINI_API_KEY (or GOOGLE_API_KEY) is not set (needed for the Gemini player)")?;
        let model = model.unwrap_or_else(|| DEFAULT_GEMINI_MODEL.to_string());
        Ok(GeminiPlayer {
            name: format!("gemini:{model}"),
            model,
            api_key,
            client: reqwest::Client::new(),
        })
    }
}

#[async_trait]
impl Player for GeminiPlayer {
    fn name(&self) -> &str {
        &self.name
    }

    async fn decide(&self, view: &PlayerView) -> anyhow::Result<Decision> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            self.model
        );
        let body = json!({
            "system_instruction": {
                "parts": [{ "text": "You are a competitive rock-paper-scissors player. Play to win." }]
            },
            "contents": [{
                "role": "user",
                "parts": [{ "text": build_prompt(view) }]
            }],
            "generationConfig": { "maxOutputTokens": 512 }
        });

        let resp = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("request to Gemini failed")?;

        let status = resp.status();
        let value: serde_json::Value = resp.json().await.context("invalid JSON from Gemini")?;
        if !status.is_success() {
            return Err(anyhow!("Gemini returned {status}: {value}"));
        }

        let text: String = value["candidates"][0]["content"]["parts"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        let mv = parse_last_move(&text)?;
        Ok(Decision {
            mv,
            note: first_line(&text),
        })
    }
}

/// The first non-empty line of model output, for display as a "note".
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{MatchState, Round, Seat};

    #[test]
    fn parse_last_move_prefers_final_line() {
        let text = "I considered rock and paper.\nscissors";
        assert_eq!(parse_last_move(text).unwrap(), Move::Scissors);
    }

    #[test]
    fn prompt_includes_history_and_score() {
        let mut state = MatchState::new("me", "you", 5);
        state.rounds.push(Round { a: Move::Rock, b: Move::Scissors });
        let prompt = build_prompt(&state.view_for(Seat::A));
        assert!(prompt.contains("round 2 of 5"));
        assert!(prompt.contains("you won"));
        assert!(prompt.contains("Score so far: you 1"));
    }
}
