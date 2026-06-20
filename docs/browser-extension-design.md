# Design: browser-extension RPS referee (Claude.ai vs Gemini)

Status: **proposal** — for review before implementation.
Chosen technique: **DOM automation** (type into the composer, read the streamed
reply from the page). Target: **Chrome MV3** (also loads in Edge; a Firefox port
is a later chore, not a rewrite).

## 1. Why an extension (and why this isn't OAuth)

The goal is the "ideal" from the first conversation: a user connects their *own*
retail Claude and Gemini accounts and watches the two play. There is no public
OAuth API for the consumer products, so a third-party web app can't do this. An
extension sidesteps the whole problem: it runs **inside the user's own browser,
as the user**, reusing the session cookies already present in their logged-in
`claude.ai` and `gemini.google.com` tabs. It is not a third-party app requesting
access; it is automation acting as the signed-in user.

This maps cleanly onto the Rust core already in the repo: the **referee loop is
unchanged**, and each chat tab is just a remote `Player` whose transport is
browser messaging instead of HTTP.

## 2. Constraints we are designing around

- **Private surface.** Selectors and DOM structure are undocumented and change
  without notice. The design must localize every site-specific assumption so a
  break is a one-file config fix, not a rewrite.
- **ToS gray area.** Automating the consumer products, even with one's own
  account, likely violates their terms. Acceptable for a personal hobby bot on
  the user's own accounts; the extension should make this explicit on first run,
  not bury it.
- **No background fetch to the chat APIs.** We deliberately do not replay
  internal endpoints (the rejected "full interception" option). All I/O goes
  through the visible page, mimicking a user.
- **Generation is asynchronous and streamed.** The hard problem is reliably
  detecting "the reply is finished," not typing the prompt.

## 3. Architecture

```
        ┌────────────────────────── Popup UI (control panel) ─────────────────────────┐
        │  start/stop match · rounds · live scoreboard · per-round log                 │
        └───────────────▲───────────────────────────────────────────────▲─────────────┘
                        │ runtime messages                               │
        ┌───────────────┴───────────────── Background service worker ────┴─────────────┐
        │  THE REFEREE: match state + history, the play_match loop, move parsing,       │
        │  scoring, persistence (chrome.storage.session). Talks to each tab's content   │
        │  script via tabs.sendMessage; aggregates the two moves into a round.          │
        └──────────────▲──────────────────────────────────────────────▲────────────────┘
                       │ tab messages                                  │
        ┌──────────────┴─────────────┐                  ┌──────────────┴─────────────┐
        │ content script @ claude.ai │                  │ content script @ gemini... │
        │ ChatDriver (DOM automation)│                  │ ChatDriver (DOM automation)│
        │  submit(prompt)→read reply │                  │  submit(prompt)→read reply │
        └────────────────────────────┘                  └────────────────────────────┘
```

Three runtime contexts, one shared `core/` library:

| Context | Role | Maps to Rust |
|---|---|---|
| Background service worker | The referee. Owns the loop and all game logic. | `referee.rs` + `game.rs` |
| Content script (per site) | A `Player`. Wraps a site-specific `ChatDriver`. | `agents.rs` (a `Player` impl) |
| Popup | Control panel + live view. | `bin/rps.rs` (the CLI driver) |

### 3.1 Relationship to the existing Rust code

The TS `core/` reimplements the **small** pure pieces — `Move`, `beats`,
lenient `parseMove`, `Round`/scoring, history→prompt rendering — directly from
`src/game.rs` and `src/agents.rs`. It is ~150 lines; porting beats WASM-compiling
the Rust for something this size. The Rust crate stays as the **API-key path**
(headless, scriptable, CI-testable) and the offline strategy benchmark. Two front
ends, one mental model. (If the core ever grows, revisit WASM to dedupe.)

## 4. Message protocol

All messages are discriminated unions on a `kind` field. Background ↔ content:

```ts
// referee → player (a tab)
type DecideRequest = {
  kind: "decide";
  requestId: string;            // correlates the async reply
  prompt: string;               // rules + full history + "end with one word"
  timeoutMs: number;
};
// player (a tab) → referee
type DecideReply =
  | { kind: "decided"; requestId: string; move: Move; raw: string }
  | { kind: "decideError"; requestId: string; reason: string; raw?: string };

// liveness / setup
type Ping = { kind: "ping" };
type Pong = { kind: "pong"; site: "claude" | "gemini"; ready: boolean };
```

Popup ↔ background:

```ts
type StartMatch = { kind: "start"; rounds: number };
type StopMatch  = { kind: "stop" };
type MatchEvent =                       // background → popup (broadcast)
  | { kind: "round"; report: RoundReport }
  | { kind: "score"; a: number; b: number; draws: number }
  | { kind: "status"; text: string }    // "waiting for Gemini…", errors, etc.
  | { kind: "done"; winner: "claude" | "gemini" | "draw" };
```

The referee never blocks on a single tab indefinitely: every `decide` carries a
`timeoutMs`; on timeout the round is recorded as an error and the match pauses
with a status message rather than hanging.

## 5. The round loop (referee)

Identical in spirit to `play_match`:

```
for n in 1..=rounds:
    promptA = render(history, seat=claude)   // same builder as agents.rs
    promptB = render(history, seat=gemini)
    [ra, rb] = await Promise.all([            // simultaneous — neither sees the
        ask(claudeTab, promptA, timeout),     // other's current move
        ask(geminiTab, promptB, timeout),
    ])
    round = { a: ra.move, b: rb.move }
    history.push(round); score(round)
    broadcast({ round })                      // popup updates live
```

`Promise.all` over two tabs gives true simultaneity for free. Each prompt
re-states the full history, so the bot is robust even if a chat thread loses its
own context or we start a fresh conversation each round.

### Conversation strategy
Default: **one ongoing conversation per site per match** (less UI churn, the
model also keeps its own memory), but **always embed explicit history** in every
prompt so correctness never depends on the chat's internal state. A "fresh
conversation each round" toggle is a later option if context bleed is a problem.

## 6. ChatDriver — the DOM automation (the crux)

Each site implements one interface; all site-specific brittleness lives here:

```ts
interface ChatDriver {
  ready(): boolean;                 // logged in, composer present, not mid-generation
  submit(text: string): Promise<void>;   // set composer text + trigger send
  awaitReply(timeoutMs: number): Promise<string>;  // resolve with final reply text
}
```

### 6.1 Submitting
- Locate the composer (a `contenteditable`/`textarea`). Set its value, dispatch
  the `input` event the framework listens for, then either click the send button
  or dispatch an Enter `keydown`.
- Guard: refuse to submit if `ready()` is false (mid-generation or logged out).

### 6.2 Detecting "reply finished" — the real problem
Streaming means the reply text keeps changing. We combine signals (first that
fires wins, with the debounce as the safety net):

1. **Control-state flip (primary).** While generating, the UI shows a *stop*
   button; when done it returns to a *send* button (often disabled until typing).
   Watch that element's state.
2. **Quiescence debounce (fallback).** A `MutationObserver` on the message list;
   when the latest assistant node hasn't mutated for ~800 ms, treat it as done.
3. **Hard timeout.** `timeoutMs` ceiling → `decideError`.

Then read the last assistant message node's `innerText`.

### 6.3 Extracting the move
Reuse the lenient parser ported from `agents.rs::parse_last_move`: prefer the
last non-empty line, fall back to first keyword anywhere. On failure, **one
clarifying retry**: send "Reply with exactly one word: rock, paper, or
scissors." If that also fails → `decideError`.

### 6.4 Keeping selectors survivable
All selectors live in a per-site config object with **ordered fallbacks** and
semantic anchors (ARIA roles, `data-testid`, placeholder text) preferred over
brittle class names:

```ts
const CLAUDE: SiteConfig = {
  composer: ['div[contenteditable="true"]', 'textarea'],
  sendButton: ['button[aria-label*="Send" i]', 'button[type="submit"]'],
  stopButton: ['button[aria-label*="Stop" i]'],
  assistantMsg: ['[data-testid*="assistant"]', '.font-claude-message'],
};
```

A `selftest` command in the popup runs `ready()` + a probe submit so the user can
confirm selectors before a match — and tells us exactly what drifted when a site
updates.

## 7. Failure modes & handling

| Failure | Detection | Response |
|---|---|---|
| Not logged in / login wall | `ready()` false, no composer | Status: "Open and log into claude.ai", pause |
| Selector drift | `submit`/`awaitReply` can't find node | `decideError` naming the missing selector; `selftest` to localize |
| Generation never ends | `timeoutMs` exceeded | `decideError`; pause match, keep history |
| Unparseable reply | parser returns none | one clarifying retry, then `decideError` |
| Captcha / rate limit | reply text/UI signals, or timeout | surface message, pause; user resolves and resumes |
| Tab closed mid-match | `tabs.sendMessage` rejects | pause, prompt user to reopen, offer resume |

The match is **pausable and resumable** from stored history — a single brittle
round never loses the game.

## 8. Permissions (MV3 manifest sketch)

```jsonc
{
  "manifest_version": 3,
  "name": "RPS Arena",
  "permissions": ["storage", "scripting", "tabs"],
  "host_permissions": ["https://claude.ai/*", "https://gemini.google.com/*"],
  "background": { "service_worker": "background.js", "type": "module" },
  "content_scripts": [
    { "matches": ["https://claude.ai/*"], "js": ["content-claude.js"] },
    { "matches": ["https://gemini.google.com/*"], "js": ["content-gemini.js"] }
  ],
  "action": { "default_popup": "popup.html" }
}
```

Minimal surface: no `<all_urls>`, no remote code, only the two chat origins.

## 9. Proposed file layout

```
extension/
  manifest.json
  src/
    core/            # ported from the Rust core; framework-free, unit-tested
      move.ts        # Move, beats, parseMove   (game.rs + agents.rs parser)
      game.ts        # Round, scoring, history → prompt
      protocol.ts    # the message union types from §4
    background/
      referee.ts     # play_match loop over two tabs
      index.ts       # wiring, storage, popup broadcast
    content/
      driver.ts      # ChatDriver interface + shared submit/await helpers
      claude.ts      # CLAUDE SiteConfig + content entry
      gemini.ts      # GEMINI SiteConfig + content entry
    popup/
      popup.html / popup.ts   # controls + live scoreboard
  tests/             # core/ unit tests (Vitest) — rules, parsing, scoring
  build: vite + @crxjs/vite-plugin (TS → MV3 bundle)
```

`core/` is pure and unit-tested exactly like the Rust core; the DOM layer is
thin and isolated so the untestable part stays small.

## 10. Incremental build plan

1. **Scaffold + core.** Vite/CRXJS MV3 project; port `core/` from Rust; Vitest
   passing on rules/parsing/scoring. Loads as an empty extension.
2. **One driver, mocked opponent.** Claude `ChatDriver` (submit + awaitReply +
   `selftest`); referee plays Claude vs a local `random`/`counter` player. Proves
   the hardest piece (reply detection) against a real site early.
3. **Second driver.** Gemini `ChatDriver`; Claude-vs-Gemini end to end.
4. **Popup.** Start/stop, rounds, live scoreboard + log, first-run ToS notice,
   `selftest` button.
5. **Resilience.** Pause/resume, clarifying-retry, timeout/error surfacing.

Milestone 2 is the go/no-go: if reliable reply-detection on a live site proves
too flaky, we reconsider the "hybrid: DOM in, network out" technique (read the
SSE stream the page already opens) without touching the referee or popup.

## 11. Open questions for review

1. **Chrome-only to start, or care about Firefox now?** (Affects manifest/build
   choices slightly; Chrome MV3 is the assumed default.)
2. **One ongoing chat per match vs. fresh conversation each round** — default is
   ongoing-with-embedded-history; fine to flip.
3. **Show each model's reasoning?** The reply often contains its rationale before
   the move; we can surface it as the per-round "note" (like the CLI does) or
   keep the log to just moves.
4. **Match format** — fixed N rounds (current) vs. first-to-K. Trivial either way.
```
