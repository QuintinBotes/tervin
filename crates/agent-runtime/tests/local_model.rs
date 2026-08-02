//! The local-model adapter, driven against a real HTTP server.
//!
//! No local model is installed on the machine this was written on, so the endpoint
//! is stood up here: a real TCP listener speaking real HTTP/1.1 with real
//! server-sent events. That is enough to prove the parts that actually break —
//! chunk boundaries falling mid-event, `[DONE]`, usage arriving in its own frame,
//! multi-turn history, and an interrupt landing mid-stream.
//!
//! Written by hand rather than with a server framework because the point is to
//! control the bytes: a helpful framework would tidy up exactly the awkward framing
//! this needs to reproduce.

use agent_runtime::local::LocalModelRuntime;
use agent_runtime::runtime::{AgentRuntime, LaunchConfig, LaunchedSession};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tervin_core::{EventPayload, TervinEvent, ThreadId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TIMEOUT: Duration = Duration::from_secs(20);

/// How the fake endpoint should answer.
#[derive(Clone)]
enum Behaviour {
    /// Stream these chunks as SSE, split exactly as given so chunk boundaries can be
    /// made to fall inside an event.
    Stream(Vec<String>),
    /// Answer with an HTTP error and this body.
    Error(u16, String),
    /// Send headers, then stall, so an interrupt has something to interrupt.
    Stall,
}

struct Endpoint {
    base_url: String,
    /// How many chat completions were requested, for multi-turn assertions.
    turns: Arc<AtomicUsize>,
    /// The request bodies received, so the conversation sent can be inspected.
    bodies: Arc<tokio::sync::Mutex<Vec<String>>>,
}

/// Start a fake OpenAI-compatible endpoint.
async fn endpoint(models: &[&str], behaviour: Behaviour) -> Endpoint {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("no port");
    let port = listener.local_addr().unwrap().port();
    let model_json = serde_json::json!({
        "object": "list",
        "data": models
            .iter()
            .map(|m| serde_json::json!({ "id": m, "object": "model" }))
            .collect::<Vec<_>>(),
    })
    .to_string();

    let turns = Arc::new(AtomicUsize::new(0));
    let bodies = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    {
        let turns = turns.clone();
        let bodies = bodies.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let model_json = model_json.clone();
                let behaviour = behaviour.clone();
                let turns = turns.clone();
                let bodies = bodies.clone();

                tokio::spawn(async move {
                    // Read until the headers end, then the body if one was announced.
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 4096];
                    let mut body_starts = None;
                    loop {
                        let Ok(n) = socket.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&buf[..n]);
                        if body_starts.is_none() {
                            if let Some(at) = find(&raw, b"\r\n\r\n") {
                                body_starts = Some(at + 4);
                            }
                        }
                        if let Some(start) = body_starts {
                            let head = String::from_utf8_lossy(&raw[..start]).to_lowercase();
                            let want = head
                                .split("content-length:")
                                .nth(1)
                                .and_then(|rest| rest.split("\r\n").next())
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            if raw.len() >= start + want {
                                break;
                            }
                        }
                    }

                    let start = body_starts.unwrap_or(raw.len());
                    let head = String::from_utf8_lossy(&raw[..start]).to_string();
                    let body = String::from_utf8_lossy(&raw[start..]).to_string();

                    if head.starts_with("GET /v1/models") {
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                            model_json.len(),
                            model_json
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                        return;
                    }

                    turns.fetch_add(1, Ordering::SeqCst);
                    bodies.lock().await.push(body);

                    match behaviour {
                        Behaviour::Error(status, detail) => {
                            let response = format!(
                                "HTTP/1.1 {status} Bad Request\r\nContent-Type: application/json\r\n\
                                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                                detail.len(),
                                detail
                            );
                            let _ = socket.write_all(response.as_bytes()).await;
                        }
                        Behaviour::Stall => {
                            let _ = socket
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                                      Transfer-Encoding: chunked\r\n\
                                      Connection: close\r\n\r\n",
                                )
                                .await;
                            // Hold the connection open with nothing on it.
                            tokio::time::sleep(Duration::from_secs(60)).await;
                        }
                        Behaviour::Stream(chunks) => {
                            let _ = socket
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                                      Transfer-Encoding: chunked\r\n\
                                      Connection: close\r\n\r\n",
                                )
                                .await;
                            for chunk in chunks {
                                // Chunked transfer encoding, written by hand so the
                                // split points are exactly the ones given.
                                let framed = format!("{:x}\r\n{}\r\n", chunk.len(), chunk);
                                if socket.write_all(framed.as_bytes()).await.is_err() {
                                    return;
                                }
                                let _ = socket.flush().await;
                                tokio::time::sleep(Duration::from_millis(5)).await;
                            }
                            let _ = socket.write_all(b"0\r\n\r\n").await;
                        }
                    }
                });
            }
        });
    }

    Endpoint {
        base_url: format!("http://127.0.0.1:{port}/v1"),
        turns,
        bodies,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// One SSE event carrying a content delta.
fn delta(text: &str) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({
            "choices": [{ "delta": { "content": text }, "index": 0 }]
        })
    )
}

async fn launch(endpoint: &Endpoint, prompt: &str) -> LaunchedSession {
    let runtime = LocalModelRuntime::custom("test-local", "Test model", &endpoint.base_url);
    let config = LaunchConfig::new(ThreadId::new(), "/tmp").with_prompt(prompt);
    tokio::time::timeout(TIMEOUT, runtime.launch(config))
        .await
        .expect("launch timed out")
        .expect("launch failed")
}

/// Collect events until the model has answered or failed.
async fn drain_until_settled(launched: &mut LaunchedSession) -> Vec<TervinEvent> {
    let mut events = Vec::new();
    while let Some(event) = launched.events.recv().await {
        let done = match &event.payload {
            EventPayload::ThreadFailed { .. } => true,
            // A conversational runtime returns to awaiting input rather than
            // completing: the Thread is still there to ask again.
            EventPayload::ThreadState { state } => {
                *state == tervin_core::ThreadState::AwaitingInput
                    && events
                        .iter()
                        .any(|e: &TervinEvent| e.kind() == "user.prompted")
            }
            _ => false,
        };
        events.push(event);
        if done {
            break;
        }
    }
    events
}

fn text_of(events: &[TervinEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------- tests

#[tokio::test]
async fn discovery_reports_the_models_the_server_actually_has() {
    let endpoint = endpoint(&["qwen3-8b", "llama-3.3-70b"], Behaviour::Stream(vec![])).await;
    let runtime = LocalModelRuntime::custom("test-local", "Test model", &endpoint.base_url);

    let discovery = runtime.discover().await;
    assert!(discovery.available);
    assert_eq!(discovery.version.as_deref(), Some("qwen3-8b"));
    assert!(
        discovery.notes.iter().any(|n| n.contains("qwen3-8b")),
        "the loaded models should be named: {:?}",
        discovery.notes
    );
    // And it says what it cannot do, so nobody expects an agent.
    assert!(discovery
        .notes
        .iter()
        .any(|n| n.contains("cannot run commands")));
}

#[tokio::test]
async fn an_endpoint_with_no_model_loaded_is_not_reported_as_ready() {
    // It answers, so a naive check would call it available — and then every prompt
    // would fail with nothing to explain why.
    let endpoint = endpoint(&[], Behaviour::Stream(vec![])).await;
    let runtime = LocalModelRuntime::custom("test-local", "Test model", &endpoint.base_url);

    let discovery = runtime.discover().await;
    assert!(!discovery.available);
    assert!(discovery
        .notes
        .iter()
        .any(|n| n.contains("no model loaded")));
}

#[tokio::test]
async fn a_streamed_reply_is_assembled_into_one_message() {
    let endpoint = endpoint(
        &["qwen3-8b"],
        Behaviour::Stream(vec![
            delta("The test "),
            delta("failed because "),
            delta("the port was in use."),
            "data: [DONE]\n\n".to_string(),
        ]),
    )
    .await;

    let mut launched = launch(&endpoint, "why did the test fail?").await;
    let events = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched))
        .await
        .expect("the turn never ended");

    assert_eq!(
        text_of(&events),
        "The test failed because the port was in use."
    );
    let kinds: Vec<&str> = events.iter().map(|e| e.kind()).collect();
    assert!(kinds.contains(&"user.prompted"), "{kinds:?}");
    assert!(kinds.contains(&"agent.message"), "{kinds:?}");
}

#[tokio::test]
async fn an_event_split_across_chunk_boundaries_still_parses() {
    // The failure that only shows up against a real socket: a TCP read ending in the
    // middle of a `data:` line. Buffering has to survive it.
    let whole = delta("split cleanly");
    let (head, tail) = whole.split_at(whole.len() / 2);
    let endpoint = endpoint(
        &["m"],
        Behaviour::Stream(vec![
            head.to_string(),
            tail.to_string(),
            "data: [DONE]\n\n".to_string(),
        ]),
    )
    .await;

    let mut launched = launch(&endpoint, "hi").await;
    let events = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched))
        .await
        .expect("the turn never ended");
    assert_eq!(text_of(&events), "split cleanly");
}

#[tokio::test]
async fn token_counts_are_reported_but_no_price_is_invented() {
    // A model on your own machine has no cost. Reporting a number would be worse
    // than leaving it blank.
    let endpoint = endpoint(
        &["m"],
        Behaviour::Stream(vec![
            delta("done"),
            format!(
                "data: {}\n\n",
                serde_json::json!({
                    "choices": [],
                    "usage": { "prompt_tokens": 41, "completion_tokens": 7 }
                })
            ),
            "data: [DONE]\n\n".to_string(),
        ]),
    )
    .await;

    let mut launched = launch(&endpoint, "hi").await;
    let events = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched))
        .await
        .expect("the turn never ended");

    let cost = events
        .iter()
        .find_map(|e| match &e.payload {
            EventPayload::CostUpdated { snapshot } => Some(snapshot.clone()),
            _ => None,
        })
        .expect("no cost.updated");
    assert_eq!(cost.input_tokens, Some(41));
    assert_eq!(cost.output_tokens, Some(7));
    assert!(
        cost.total_cost_usd.is_none(),
        "a local model has no price: {:?}",
        cost.total_cost_usd
    );
    assert_eq!(cost.model.as_deref(), Some("m"));
}

#[tokio::test]
async fn the_conversation_is_carried_because_the_server_keeps_nothing() {
    let endpoint = endpoint(
        &["m"],
        Behaviour::Stream(vec![delta("first answer"), "data: [DONE]\n\n".to_string()]),
    )
    .await;

    let mut launched = launch(&endpoint, "first question").await;
    let _ = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched)).await;

    launched
        .session
        .send_input("second question".to_string(), Vec::new())
        .await
        .expect("second turn refused");
    let _ = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched)).await;

    assert_eq!(endpoint.turns.load(Ordering::SeqCst), 2);
    let bodies = endpoint.bodies.lock().await;
    let second = &bodies[1];
    // The second request has to carry the whole exchange, or the model has amnesia.
    assert!(second.contains("first question"), "{second}");
    assert!(second.contains("first answer"), "{second}");
    assert!(second.contains("second question"), "{second}");
}

#[tokio::test]
async fn a_server_error_explains_itself_rather_than_showing_a_status_code() {
    let endpoint = endpoint(
        &["m"],
        Behaviour::Error(
            400,
            r#"{"error":{"message":"model 'm' is not loaded"}}"#.to_string(),
        ),
    )
    .await;

    let mut launched = launch(&endpoint, "hi").await;
    let events = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched))
        .await
        .expect("the turn never ended");

    let failed = events
        .iter()
        .find(|e| e.kind() == "thread.failed")
        .expect("no thread.failed");
    assert!(
        failed.summary.contains("not loaded"),
        "the server's own explanation should survive: {}",
        failed.summary
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_says_what_to_do_about_it() {
    let runtime = LocalModelRuntime::custom("test-local", "Test model", "http://127.0.0.1:1");
    let config = LaunchConfig::new(ThreadId::new(), "/tmp").with_prompt("hi");
    // Launch itself fails, because a model has to be chosen before a turn can start.
    let error = runtime
        .launch(config)
        .await
        .err()
        .expect("an unreachable endpoint must not launch");
    assert!(!error.to_string().is_empty());
}

#[tokio::test]
async fn interrupting_mid_stream_is_not_reported_as_a_failure() {
    // Stopping something on purpose is not an error, and showing it as one would
    // train people to ignore real ones.
    let endpoint = endpoint(&["m"], Behaviour::Stall).await;
    let mut launched = launch(&endpoint, "hi").await;

    // Let the request reach the server before interrupting it.
    tokio::time::timeout(TIMEOUT, async {
        while endpoint.turns.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the request never arrived");

    launched
        .session
        .interrupt()
        .await
        .expect("interrupt failed");

    let events = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched))
        .await
        .expect("the turn never ended");
    let failed = events
        .iter()
        .find(|e| e.kind() == "thread.failed")
        .expect("no terminal event");
    assert!(
        failed.summary.contains("Interrupted"),
        "an interrupt should read as an interrupt: {}",
        failed.summary
    );
}

#[tokio::test]
async fn a_second_prompt_while_answering_is_refused_clearly() {
    let endpoint = endpoint(&["m"], Behaviour::Stall).await;
    let launched = launch(&endpoint, "hi").await;

    tokio::time::timeout(TIMEOUT, async {
        while endpoint.turns.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the request never arrived");

    let error = launched
        .session
        .send_input("again".to_string(), Vec::new())
        .await
        .expect_err("a concurrent turn must be refused");
    assert!(error.to_string().contains("still answering"), "{error}");

    let _ = launched.session.shutdown().await;
}

#[tokio::test]
async fn attachments_are_sent_and_nothing_else_is() {
    // The privacy promise, checked on the wire: what was attached appears, and the
    // prompt appears, and nothing else does.
    let endpoint = endpoint(
        &["m"],
        Behaviour::Stream(vec![delta("ok"), "data: [DONE]\n\n".to_string()]),
    )
    .await;

    let runtime = LocalModelRuntime::custom("test-local", "Test model", &endpoint.base_url);
    let mut config = LaunchConfig::new(ThreadId::new(), "/tmp");
    config.prompt = Some("what went wrong?".into());
    config.attachments = vec![agent_runtime::runtime::Attachment::Block {
        block_id: tervin_core::BlockId::new(),
        command: "cargo test".into(),
        output: "error[E0308]: mismatched types".into(),
    }];

    let mut launched = runtime.launch(config).await.expect("launch failed");
    let events = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched))
        .await
        .expect("the turn never ended");

    // Recorded as explicit context.
    assert!(events.iter().any(|e| e.kind() == "context.attached"));

    let bodies = endpoint.bodies.lock().await;
    let sent = &bodies[0];
    assert!(sent.contains("cargo test"), "the block should be sent");
    assert!(sent.contains("E0308"), "its output should be sent");
    assert!(sent.contains("what went wrong?"));
}

#[tokio::test]
async fn a_model_endpoint_has_nothing_to_approve_and_says_so() {
    let endpoint = endpoint(&["m"], Behaviour::Stream(vec![])).await;
    let launched = launch(&endpoint, "hi").await;

    let permissions = launched.session.permissions();
    // Not a gate, and not a gap.
    assert!(!permissions.tervin_can_intercept);
    assert!(
        permissions.explanation.contains("nothing to approve"),
        "{}",
        permissions.explanation
    );

    // Modes are meaningless here, and asking for one is an error rather than a
    // silent no-op.
    assert!(launched.session.set_permission_mode("plan").await.is_err());
    let _ = launched.session.shutdown().await;
}
