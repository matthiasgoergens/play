//! Side-by-side web viewer. Runs a match and streams each round to the browser
//! over Server-Sent Events, so a human can watch both models' commentary unfold
//! next to each other in real time.
//!
//! `GET /`        -> the two-column page.
//! `GET /stream`  -> SSE; query params: rounds, p1, p2 (player specs).

use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::Query,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html,
    },
    routing::get,
    Router,
};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_stream::{wrappers::UnboundedReceiverStream, Stream, StreamExt};

use rps::game::Outcome;
use rps::referee::play_match;
use rps::transcript::Recorder;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/stream", get(stream));

    let addr = "127.0.0.1:8080";
    let listener = TcpListener::bind(addr).await?;
    println!("RPS arena: open http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../../static/index.html"))
}

#[derive(serde::Deserialize)]
struct Params {
    rounds: Option<usize>,
    p1: Option<String>,
    p2: Option<String>,
}

fn outcome_str(o: Outcome) -> &'static str {
    match o {
        Outcome::Win => "win",
        Outcome::Loss => "loss",
        Outcome::Draw => "draw",
    }
}

async fn stream(Query(p): Query<Params>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rounds = p.rounds.unwrap_or(10).clamp(1, 100);
    let p1 = p.p1.unwrap_or_else(|| "anthropic".to_string());
    let p2 = p.p2.unwrap_or_else(|| "gemini".to_string());

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    tokio::spawn(async move {
        let players = (
            rps::build_player(&p1, None),
            rps::build_player(&p2, None),
        );
        let (a, b) = match players {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                let _ = tx.send(json!({ "type": "error", "message": format!("{e:#}") }).to_string());
                return;
            }
        };

        let name_a = a.name().to_string();
        let name_b = b.name().to_string();

        let recorder = Recorder::create(Path::new("games"), &name_a, &name_b)
            .map(Arc::new)
            .ok();
        let dir = recorder
            .as_ref()
            .map(|r| r.dir().display().to_string())
            .unwrap_or_default();

        let _ = tx.send(
            json!({ "type": "start", "a": name_a, "b": name_b, "rounds": rounds, "dir": dir })
                .to_string(),
        );

        let mut score = (0usize, 0usize, 0usize); // wins_a, wins_b, draws
        let tx_round = tx.clone();
        let na = name_a.clone();
        let nb = name_b.clone();

        let result = play_match(a.as_ref(), b.as_ref(), rounds, recorder.as_deref(), |r| {
            let outcome_b = match r.outcome_a {
                Outcome::Win => Outcome::Loss,
                Outcome::Loss => Outcome::Win,
                Outcome::Draw => Outcome::Draw,
            };
            match r.outcome_a {
                Outcome::Win => score.0 += 1,
                Outcome::Loss => score.1 += 1,
                Outcome::Draw => score.2 += 1,
            }
            let event = json!({
                "type": "round",
                "round": r.number,
                "a": {
                    "name": na,
                    "move": r.move_a.as_str(),
                    "reasoning": r.note_a,
                    "outcome": outcome_str(r.outcome_a),
                },
                "b": {
                    "name": nb,
                    "move": r.move_b.as_str(),
                    "reasoning": r.note_b,
                    "outcome": outcome_str(outcome_b),
                },
                "score": { "a": score.0, "b": score.1, "draws": score.2 },
            });
            let _ = tx_round.send(event.to_string());
        })
        .await;

        match result {
            Ok(_) => {
                if let Some(rec) = &recorder {
                    let _ = rec.finish(None);
                }
                let _ = tx.send(
                    json!({ "type": "done",
                            "score": { "a": score.0, "b": score.1, "draws": score.2 } })
                    .to_string(),
                );
            }
            Err(e) => {
                if let Some(rec) = &recorder {
                    let _ = rec.finish(Some(&format!("{e:#}")));
                }
                let _ = tx.send(
                    json!({ "type": "error", "message": format!("{e:#}") }).to_string(),
                );
            }
        }
    });

    let stream = UnboundedReceiverStream::new(rx)
        .map(|data| Ok::<_, Infallible>(Event::default().data(data)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}
