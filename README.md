# rps — watch two AIs read each other over rock-paper-scissors

A small Rust referee that automates what you were doing by hand: it asks two
agents (**Claude** and **Gemini**) for a move each round, reveals them
simultaneously, and repeats. Only the **moves** are relayed between the two
models — never each other's words — so anything each says about the other is
inferred from play alone.

The point isn't the score. It's watching the models think out loud and
spontaneously start profiling each other from round 1. Each model keeps its own
running conversation, so its theory of the opponent develops as the match goes
on. The **side-by-side web viewer** puts both streams next to each other.

Local (non-network) players are also built in, so the harness runs today without
any API key.

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

The framing is deliberately light: each model is told only that it's playing an
iterated match against *another AI* and is invited to think out loud, then end
with its move. It is never told to "analyze" or "win" — that behaviour shows up
on its own. Each model keeps its own conversation thread, so it remembers its
earlier reads and they evolve over the match.

## Side-by-side viewer

The best way to watch. Serves a two-column page (left model vs right model) and
streams each round's full commentary as it lands.

```bash
export ANTHROPIC_API_KEY=...  GEMINI_API_KEY=...
cargo run --bin rps-serve         # then open http://127.0.0.1:8080
```

Set the two player specs and the round count in the page header and press Start.
It works with the local players too (`counter`, `random`, …) for a keyless
smoke test of the plumbing.

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
- `src/bin/rps.rs` — the CLI driver.
- `src/bin/serve.rs` + `static/index.html` — the side-by-side web viewer (SSE).

`Player` is the only seam that matters: anything that can produce a move from a
history is a contestant. `cargo test` covers the rules, parsing, and a
deterministic `counter`-vs-`fixed` match.

## Connecting agents — API keys, not OAuth

The models are driven through their **developer APIs** with **API keys** you
supply (`ANTHROPIC_API_KEY`, `GEMINI_API_KEY`). There is no public OAuth API for
the *retail* products (Claude.ai, the Gemini app) that lets a third-party app
drive their chat, so a hosted "log in with your account" version isn't possible
that way.

The one route that could pit *retail* accounts against each other is a **browser
extension** that automates your own logged-in tabs — recorded as an alternative
in [`docs/browser-extension-design.md`](docs/browser-extension-design.md), not
currently built.

## Roadmap

- Stream each model's commentary token-by-token (not just per-round) for an even
  more live feel.
- Optionally let each model see the opponent's words too (a different game —
  open negotiation rather than reading moves cold).
- More strategy players (Markov predictor, etc.) for offline benchmarking.
