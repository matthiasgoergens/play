//! LLM-backed players. Each adapter holds its *own* running conversation, so a
//! model's commentary builds across rounds — its developing read of the
//! opponent is the whole point. Only moves are relayed between the two models
//! (never each other's reasoning), so any psychoanalysis is inferred from play
//! alone, exactly like the manual prototype.
//!
//! No official Anthropic/Google SDK exists for Rust, so these talk raw HTTP via
//! `reqwest`. API keys are read from the environment; nothing is hard-coded.

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Mutex;

use crate::game::{Move, Outcome, PlayerView};
use crate::player::{Decision, Player};

/// Deliberately light-touch framing: it tells each model the opponent is another
/// AI and invites it to think out loud, but never instructs it to "analyze" or
/// "win". The interesting behaviour emerges on its own.
const SYSTEM: &str = "You're an AI language model playing an iterated game of \
rock-paper-scissors against another AI language model. Think out loud as you \
play — share whatever you're noticing or reasoning about. End every message \
with your move on its own final line: just the single word rock, paper, or \
scissors.";

#[derive(Clone, Copy)]
enum Role {
    User,
    Assistant,
}

/// The per-round user message. The model remembers the rest of the match in its
/// own conversation, so each turn only states the latest result.
fn round_user_message(view: &PlayerView) -> String {
    match view.history.last() {
        None => format!(
            "Let's play {n} rounds of rock-paper-scissors, head to head. Each round \
             we both reveal at the same time. This is round 1 of {n}. Make your move.",
            n = view.total_rounds
        ),
        Some(last) => {
            let result = match last.outcome {
                Outcome::Win => "you won",
                Outcome::Loss => "you lost",
                Outcome::Draw => "it was a draw",
            };
            format!(
                "Round {prev} result: you threw {mine}, your opponent threw {theirs} \
                 — {result}. On to round {cur} of {total}. Your move.",
                prev = view.round_number - 1,
                cur = view.round_number,
                total = view.total_rounds,
                mine = last.mine,
                theirs = last.theirs,
            )
        }
    }
}

/// Parse a move from free-form model text, preferring the last keyword (we ask
/// the model to put its decision on the final line).
fn parse_last_move(text: &str) -> anyhow::Result<Move> {
    if let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty()) {
        if let Some(mv) = Move::parse(line) {
            return Ok(mv);
        }
    }
    Move::parse(text).ok_or_else(|| anyhow!("could not find a move in model output: {text:?}"))
}

/// Push a user turn and return a snapshot of the whole conversation, without
/// holding the lock across the network call.
fn push_user(convo: &Mutex<Vec<(Role, String)>>, text: String) -> Vec<(Role, String)> {
    let mut g = convo.lock().unwrap();
    g.push((Role::User, text));
    g.clone()
}

fn push_assistant(convo: &Mutex<Vec<(Role, String)>>, text: String) {
    convo.lock().unwrap().push((Role::Assistant, text));
}

const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-4-8";
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";

/// Claude via the Messages API (`POST /v1/messages`).
pub struct AnthropicPlayer {
    name: String,
    model: String,
    api_key: String,
    client: reqwest::Client,
    convo: Mutex<Vec<(Role, String)>>,
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
            convo: Mutex::new(Vec::new()),
        })
    }

    async fn complete(&self, convo: &[(Role, String)]) -> anyhow::Result<String> {
        let messages: Vec<_> = convo
            .iter()
            .map(|(role, text)| {
                let role = match role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                json!({ "role": role, "content": text })
            })
            .collect();

        let body = json!({
            "model": self.model,
            "max_tokens": 1024,
            "system": SYSTEM,
            "messages": messages,
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

        Ok(value["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default())
    }
}

#[async_trait]
impl Player for AnthropicPlayer {
    fn name(&self) -> &str {
        &self.name
    }

    async fn decide(&self, view: &PlayerView) -> anyhow::Result<Decision> {
        let snapshot = push_user(&self.convo, round_user_message(view));
        let text = self.complete(&snapshot).await?;
        let mv = parse_last_move(&text)?;
        push_assistant(&self.convo, text.clone());
        Ok(Decision { mv, note: Some(text) })
    }
}

/// Gemini via the Generative Language API (`:generateContent`).
pub struct GeminiPlayer {
    name: String,
    model: String,
    api_key: String,
    client: reqwest::Client,
    convo: Mutex<Vec<(Role, String)>>,
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
            convo: Mutex::new(Vec::new()),
        })
    }

    async fn complete(&self, convo: &[(Role, String)]) -> anyhow::Result<String> {
        let contents: Vec<_> = convo
            .iter()
            .map(|(role, text)| {
                let role = match role {
                    Role::User => "user",
                    Role::Assistant => "model",
                };
                json!({ "role": role, "parts": [{ "text": text }] })
            })
            .collect();

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            self.model
        );
        let body = json!({
            "system_instruction": { "parts": [{ "text": SYSTEM }] },
            "contents": contents,
            "generationConfig": { "maxOutputTokens": 1024 }
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

        Ok(value["candidates"][0]["content"]["parts"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default())
    }
}

#[async_trait]
impl Player for GeminiPlayer {
    fn name(&self) -> &str {
        &self.name
    }

    async fn decide(&self, view: &PlayerView) -> anyhow::Result<Decision> {
        let snapshot = push_user(&self.convo, round_user_message(view));
        let text = self.complete(&snapshot).await?;
        let mv = parse_last_move(&text)?;
        push_assistant(&self.convo, text.clone());
        Ok(Decision { mv, note: Some(text) })
    }
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
    fn first_round_message_is_an_invitation() {
        let state = MatchState::new("me", "you", 5);
        let msg = round_user_message(&state.view_for(Seat::A));
        assert!(msg.contains("round 1 of 5"));
        assert!(msg.contains("Make your move"));
    }

    #[test]
    fn later_round_message_reports_last_result() {
        let mut state = MatchState::new("me", "you", 5);
        state.rounds.push(Round { a: Move::Rock, b: Move::Scissors });
        let msg = round_user_message(&state.view_for(Seat::A));
        assert!(msg.contains("you threw rock"));
        assert!(msg.contains("opponent threw scissors"));
        assert!(msg.contains("you won"));
        assert!(msg.contains("round 2 of 5"));
    }
}
