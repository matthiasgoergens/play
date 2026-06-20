# rps — iterated rock-paper-scissors between agents

A small Rust referee that automates what you were doing by hand: it asks two
agents for a move each round, reveals them simultaneously, scores the round, and
repeats — feeding each agent the **full move history** so it can adapt.

The two agents in your prototype were **Gemini** and **Claude**; both are built
in. So are local (non-network) players you can run today without any API key.

## Quick start (no API keys needed)

```bash
cargo run -- --p1 counter --p2 fixed:rock --rounds 8
```

```
Round  2: counter = paper  fixed:rock = rock  -> counter wins
...
Final score: counter 7 — 0 fixed:rock (draws: 1)
```

## Claude vs Gemini

Provide your own API keys via environment variables, then point the two seats at
the providers:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
export GEMINI_API_KEY=...           # or GOOGLE_API_KEY

cargo run -- --p1 anthropic --p2 gemini --rounds 10
```

Pin specific models with `kind:model`:

```bash
cargo run -- --p1 anthropic:claude-opus-4-8 --p2 gemini:gemini-2.5-pro --rounds 20
```

Each agent is sent the rules, the running scoreboard, and the full per-round
history (its own and the opponent's moves), and is asked to end its reply with a
single move word, which is parsed leniently.

## Player specs

| Spec | Description |
|------|-------------|
| `random` | Uniformly random — the unexploitable baseline. |
| `counter` | Plays to beat the opponent's most frequent move so far. |
| `fixed:rock` | Always throws the given move (`rock`/`paper`/`scissors`). |
| `anthropic[:model]` | Claude via the Messages API. Needs `ANTHROPIC_API_KEY`. |
| `gemini[:model]` | Gemini via the Generative Language API. Needs `GEMINI_API_KEY`. |

`--seed N` makes the local `random`/`counter` players reproducible.

## How the pieces fit

- `src/game.rs` — pure rules, scoring, and the per-seat `PlayerView` handed to agents.
- `src/player.rs` — the `Player` trait and the local players.
- `src/agents.rs` — the Claude and Gemini adapters (raw HTTP via `reqwest`).
- `src/referee.rs` — drives the match, querying both players concurrently each round.
- `src/bin/rps.rs` — the CLI.

`Player` is the only seam that matters: anything that can produce a move from a
history is a contestant. `cargo test` covers the rules, parsing, and a
deterministic `counter`-vs-`fixed` match.

## On "connect your own agent" (the website idea)

The eventual goal is a website where a user connects their own agent and watches
matches. Worth knowing up front: there is **no public OAuth API for the *retail*
products** (Claude.ai, the Gemini app) that lets a third-party app drive their
chat. The real connection path is the **developer APIs**, which authenticate with
**API keys** — which is what the adapters here use. The website version would
have users paste their own key (kept in their session), reusing these exact
adapters. If a retail OAuth path ever appears, it slots in as one more `Player`
implementation; nothing else has to change.

## Roadmap

- HTTP/WebSocket server wrapping `play_match` so a browser can stream rounds.
- A TypeScript frontend: configure the two players, start a match, watch the
  scoreboard update live.
- More strategy players (Markov predictor, etc.) for offline benchmarking.
