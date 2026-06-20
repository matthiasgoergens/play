//! LLM-backed players. Each adapter holds its *own* running conversation, so a
//! model's commentary builds across rounds. Only moves are relayed between the
//! two models (never each other's words), exactly like the manual prototype.
//!
//! The prompting is deliberately minimal — a mediator framing and nothing more.
//! The models were never asked to explain themselves; that they do is the fun
//! part, so we don't manufacture it.
//!
//! No official Anthropic/Google SDK exists for Rust, so these talk raw HTTP via
//! `reqwest`. API keys are read from the environment; nothing is hard-coded.

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Mutex;

use crate::game::{Move, PlayerView};
use crate::player::{Decision, Player};

#[derive(Clone, Copy)]
enum Role {
    User,
    Assistant,
}

/// A friendly name for the opponent, as a human mediator would say it
/// ("...between you and Claude"), derived from the opponent's player spec.
fn friendly(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("anthropic") || lower.starts_with("claude") {
        "Claude"
    } else if lower.starts_with("gemini") || lower.starts_with("google") {
        "Gemini"
    } else {
        name
    }
}

/// The per-round user message, in a mediator's voice. The model remembers the
/// match in its own conversation, so each turn only relays the opponent's last
/// move — not the outcome, which the model can work out itself.
fn round_user_message(view: &PlayerView) -> String {
    let opp = friendly(&view.opponent_name);
    match view.history.last() {
        None => format!(
            "I'm mediating a game of rock / scissors / paper between you and {opp}. \
             What's your first move?"
        ),
        Some(last) => format!("{opp} played {}. What's your next move?", last.theirs),
    }
}

/// Pull the chosen move out of a free-form reply (last move mentioned wins).
fn parse_reply(text: &str) -> anyhow::Result<Move> {
    Move::parse_decision(text)
        .ok_or_else(|| anyhow!("could not find a move in model output: {text:?}"))
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
        let mv = parse_reply(&text)?;
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
        let mv = parse_reply(&text)?;
        push_assistant(&self.convo, text.clone());
        Ok(Decision { mv, note: Some(text) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{MatchState, Round, Seat};

    #[test]
    fn parse_reply_takes_the_decision() {
        let text = "Claude opened with rock, so I'll answer with paper.";
        assert_eq!(parse_reply(text).unwrap(), Move::Paper);
    }

    #[test]
    fn first_message_is_the_mediator_framing() {
        let state = MatchState::new("me", "gemini:flash", 5);
        let msg = round_user_message(&state.view_for(Seat::A));
        assert!(msg.contains("between you and Gemini"));
        assert!(msg.contains("first move"));
        // No round count and no "end with your move" coaching.
        assert!(!msg.contains("round 1"));
    }

    #[test]
    fn later_message_relays_opponent_move_only() {
        let mut state = MatchState::new("me", "anthropic:claude-opus-4-8", 5);
        state.rounds.push(Round { a: Move::Rock, b: Move::Scissors });
        let msg = round_user_message(&state.view_for(Seat::A));
        assert!(msg.contains("Claude played scissors"));
        assert!(msg.contains("next move"));
        // We don't tell them whether they won.
        assert!(!msg.to_lowercase().contains("you won"));
    }
}
