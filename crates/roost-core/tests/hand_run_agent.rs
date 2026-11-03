//! A hand-run agent session against a real Ollama, to see how a real model
//! uses the tools (does it pick the right one, are the arguments sensible, are
//! results cut sensibly). It only reads and writes a throwaway folder and only
//! asks Ollama for chat replies; it never creates, copies or deletes a model.
//!
//! ```text
//! ROOST_HAND_MODEL=qwen2.5-coder:7b \
//!   cargo test -p roost-core --test hand_run_agent -- --ignored --nocapture
//! ```
//!
//! Environment: `ROOST_HAND_URL` (default `http://127.0.0.1:11434`),
//! `ROOST_HAND_MODEL` (required), `ROOST_HAND_ONLY` (comma-separated task ids,
//! default all), `ROOST_HAND_STEPS` (step budget, default 20).

use roost_core::agent::ask::{Answer, QuestionKind};
use roost_core::chat::{
    ApprovalDecision, ChatCommand, ChatConfig, ChatEvent, ChatManager, RunOutcome,
};
use roost_core::ollama::OllamaClient;
use roost_core::session::{SessionId, SessionLifetime};
use roost_core::store::{Store, Writer};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// (id, what it exercises, the prompt)
const TASKS: &[(&str, &str, &str)] = &[
    (
        "overview",
        "list_dir, read_file",
        "What is in this folder? Give me a two-sentence summary of the project.",
    ),
    (
        "todos",
        "search_text",
        "Find every TODO comment in the project and list the file and line number of each.",
    ),
    (
        "constant",
        "search_text",
        "Which file defines retry_limit? Quote the line.",
    ),
    (
        "binary",
        "find_files / read_binary",
        "There is a binary file somewhere in this folder. Which one is it, and what are its first 8 bytes in hex?",
    ),
    (
        "count",
        "find_files",
        "How many Rust files (.rs) are there under src, including subfolders?",
    ),
    (
        "needle",
        "search_text on a big file",
        "In data/big.txt, which line number contains the text needle-4242? Do not read the whole file.",
    ),
    (
        "ask",
        "ask_user",
        "Ask me which of README.md or notes.md to summarize (let me pick one of the two), then summarize the one I pick in one sentence.",
    ),
    (
        "write",
        "write_file, edit_file",
        "Create a file called summary.txt containing a one-line draft summary of README.md. Then change the word draft in summary.txt to final.",
    ),
];

fn make_workspace(dir: &Path) {
    let w = |rel: &str, text: &str| {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    w(
        "README.md",
        "# Pigeon\n\nA tiny message relay written in Rust. This is a draft of the docs.\n\n## Usage\n\nRun `pigeon serve` to start the relay.\n",
    );
    w(
        "notes.md",
        "Meeting notes\n\n- decide on the retry policy\n- move the config loader into util\n",
    );
    w(
        "src/main.rs",
        "const retry_limit: u32 = 5;\n\nfn main() {\n    // TODO: handle errors from the relay\n    pigeon::serve(retry_limit);\n}\n",
    );
    w("src/lib.rs", "pub mod util;\n\npub fn serve(_n: u32) {}\n");
    w(
        "src/util/mod.rs",
        "pub mod helpers;\n\n// TODO: split this module\npub fn clamp(x: i32) -> i32 { x.max(0) }\n",
    );
    w(
        "src/util/helpers.rs",
        "pub fn double(x: i32) -> i32 { x * 2 }\n",
    );
    let mut big = String::new();
    for n in 1..=5000 {
        if n == 4242 {
            big.push_str("line 4242: the needle-4242 is here\n");
        } else {
            big.push_str(&format!("line {n}: lorem ipsum dolor sit amet\n"));
        }
    }
    w("data/big.txt", &big);
    let mut blob = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
    blob.extend((0u8..=255).cycle().take(2000));
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/blob.bin"), blob).unwrap();
}

fn short(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        format!("{}…", one.chars().take(max).collect::<String>())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a real Ollama with a tool-capable model; see the file header"]
async fn a_real_model_uses_the_tools() {
    let model = std::env::var("ROOST_HAND_MODEL").expect("set ROOST_HAND_MODEL");
    let url = std::env::var("ROOST_HAND_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let only: Option<Vec<String>> = std::env::var("ROOST_HAND_ONLY")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());
    let budget: u32 = std::env::var("ROOST_HAND_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let store_dir = tempfile::TempDir::new().unwrap();
    let (store, warnings) = Store::open(store_dir.path()).unwrap();
    let writer = Writer::spawn(store, warnings);
    let (tx, mut rx) = mpsc::unbounded_channel::<ChatEvent>();
    let handle = ChatManager::spawn(
        &tokio::runtime::Handle::current(),
        writer.clone(),
        ChatConfig::new(OllamaClient::new(&url)),
        Arc::new(move |e| {
            let _ = tx.send(e);
        }),
    );
    handle.send(ChatCommand::SetWritesAllowed(true));
    println!("model {model} at {url}, step budget {budget}\n");

    for (task, exercises, prompt) in TASKS {
        if only.as_ref().is_some_and(|o| !o.iter().any(|t| t == task)) {
            continue;
        }
        let ws = tempfile::TempDir::new().unwrap();
        make_workspace(ws.path());
        handle.send(ChatCommand::CreateAgent {
            lifetime: SessionLifetime::Persisted,
            model: model.clone(),
            workspace: Some(ws.path().to_path_buf()),
        });
        let id = loop {
            match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
                Ok(Some(ChatEvent::Opened(s))) => break s.id,
                Ok(Some(_)) => {}
                other => panic!("no session opened: {other:?}"),
            }
        };
        handle.send(ChatCommand::SetStepBudget(id, budget));
        let started = Instant::now();
        handle.send(ChatCommand::Send {
            session: id,
            text: (*prompt).into(),
        });
        let (outcome, message) = wait_for_the_run(&handle, &mut rx, id).await;
        writer.flush().await;
        report(
            task,
            exercises,
            prompt,
            store_dir.path(),
            id,
            started.elapsed(),
            outcome,
            message,
        );
        handle.send(ChatCommand::Delete(id));
    }
    handle.shutdown().await;
}

/// Answers whatever the agent asks (the first option, "yes") and returns how
/// the run ended.
async fn wait_for_the_run(
    handle: &roost_core::chat::ChatHandle,
    rx: &mut mpsc::UnboundedReceiver<ChatEvent>,
    id: SessionId,
) -> (Option<RunOutcome>, Option<String>) {
    let deadline = Instant::now() + Duration::from_secs(900);
    while Instant::now() < deadline {
        let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(120), rx.recv()).await
        else {
            return (None, Some("no event for 120 s".into()));
        };
        match event {
            ChatEvent::ApprovalRequested {
                session, call_id, ..
            } if session == id => handle.send(ChatCommand::ResolveApproval {
                session,
                call_id,
                decision: ApprovalDecision::Allow,
            }),
            ChatEvent::QuestionAsked {
                session,
                call_id,
                question,
            } if session == id => {
                let answer = match question.kind {
                    QuestionKind::Text => Answer {
                        picked: Vec::new(),
                        other: Some("the first one".into()),
                    },
                    _ => Answer {
                        picked: question.options.first().cloned().into_iter().collect(),
                        other: None,
                    },
                };
                println!("    (asked: {:?} -> {:?})", question.text, answer.picked);
                handle.send(ChatCommand::AnswerQuestion {
                    session,
                    call_id,
                    answer: Some(answer),
                });
            }
            ChatEvent::RunEnded {
                session,
                outcome,
                message,
            } if session == id => return (Some(outcome), message),
            _ => {}
        }
    }
    (None, Some("took longer than 15 minutes".into()))
}

#[allow(clippy::too_many_arguments)]
fn report(
    task: &str,
    exercises: &str,
    prompt: &str,
    store_dir: &Path,
    id: SessionId,
    took: Duration,
    outcome: Option<RunOutcome>,
    message: Option<String>,
) {
    println!("=== {task}  [{exercises}]");
    println!("    prompt: {prompt}");
    let file = store_dir.join(format!("{id}.json"));
    let json: serde_json::Value = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let messages = json["session"]["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let (mut steps, mut calls) = (0, 0);
    let mut last_text = String::new();
    for m in &messages {
        match m["role"].as_str() {
            Some("assistant") => {
                steps += 1;
                if let Some(t) = m["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    last_text = t.to_string();
                }
            }
            Some("tool") => {
                calls += 1;
                println!(
                    "    tool {} {}  -> {} {}",
                    m["name"].as_str().unwrap_or("?"),
                    short(&m["arguments"].to_string(), 160),
                    m["status"].as_str().unwrap_or("?"),
                    short(m["output"].as_str().unwrap_or(""), 120),
                );
            }
            _ => {}
        }
    }
    println!(
        "    run: {outcome:?} {} | {steps} model turns, {calls} tool calls, {:.0} s",
        message.unwrap_or_default(),
        took.as_secs_f32()
    );
    println!("    answer: {}\n", short(&last_text, 500));
}
