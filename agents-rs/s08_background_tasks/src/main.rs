//! Harness: background execution -- the model thinks while the harness waits.
//!
//! Run commands in background threads. A notification queue is drained
//! before each LLM call to deliver results.

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef,
    assistant_message, tool_result_message, user_text, base_tools};
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Mutex;
use tokio::sync::mpsc;

struct BgTask {
    status: String,
    result: Option<String>,
    command: String,
}

struct BackgroundManager {
    tasks: Mutex<HashMap<String, BgTask>>,
    tx: mpsc::UnboundedSender<serde_json::Value>,
    rx: Mutex<mpsc::UnboundedReceiver<serde_json::Value>>,
}

impl BackgroundManager {
    fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tasks: Mutex::new(HashMap::new()),
            tx,
            rx: Mutex::new(rx),
        }
    }

    fn run(&self, command: &str, workdir: std::path::PathBuf, timeout_secs: u64) -> String {
        let task_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        self.tasks.lock().unwrap().insert(task_id.clone(), BgTask {
            status: "running".to_string(),
            result: None,
            command: command.to_string(),
        });
        let tid = task_id.clone();
        let cmd = command.to_string();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let output = harness::tools::run_bash_with_timeout(&cmd, &workdir, timeout_secs);
            // Note: run_bash_with_timeout is sync, so we wrap it
            if let Some(_task) = tx.send(serde_json::json!({
                "task_id": tid,
                "status": "completed",
                "command": &cmd[..cmd.len().min(80)],
                "result": &output[..output.len().min(500)]
            })).ok() {
            }
        });
        format!("Background task {task_id} started: {}", &command[..command.len().min(80)])
    }

    fn check(&self, task_id: Option<&str>) -> String {
        let tasks = self.tasks.lock().unwrap();
        if let Some(tid) = task_id {
            if let Some(t) = tasks.get(tid) {
                let result = t.result.as_deref().unwrap_or("(running)");
                return format!("[{}] {}\n{}", t.status, &t.command[..t.command.len().min(60)], result);
            }
            return format!("Error: Unknown task {tid}");
        }
        if tasks.is_empty() {
            return "No background tasks.".to_string();
        }
        let lines: Vec<String> = tasks.iter()
            .map(|(tid, t)| format!("{tid}: [{}] {}", t.status, &t.command[..t.command.len().min(60)]))
            .collect();
        lines.join("\n")
    }

    fn drain_notifications(&self) -> Vec<serde_json::Value> {
        let mut rx = self.rx.lock().unwrap();
        let mut notifs = Vec::new();
        while let Ok(n) = rx.try_recv() {
            notifs.push(n);
        }
        notifs
    }
}

fn background_run_tool() -> ToolDef {
    ToolDef {
        name: "background_run".to_string(),
        description: "Run command in background thread. Returns task_id immediately.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"]
        }),
    }
}

fn check_background_tool() -> ToolDef {
    ToolDef {
        name: "check_background".to_string(),
        description: "Check background task status. Omit task_id to list all.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"task_id": {"type": "string"}}
        }),
    }
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &std::path::Path,
    bg: &BackgroundManager,
) {
    let system = format!("You are a coding agent at {}. Use background_run for long-running commands.", workdir.display());
    let mut tools = base_tools();
    tools.push(background_run_tool());
    tools.push(check_background_tool());

    loop {
        // Drain background notifications
        let notifs = bg.drain_notifications();
        if !notifs.is_empty() && !messages.is_empty() {
            let notif_text: Vec<String> = notifs.iter()
                .map(|n| format!("[bg:{}] {}: {}", n["task_id"], n["status"], n["result"]))
                .collect();
            messages.push(user_text(&format!("<background-results>\n{}\n</background-results>", notif_text.join("\n"))));
        }

        let resp = match client.create_message(&system, messages, &tools, 8000).await {
            Ok(r) => r,
            Err(e) => { eprintln!("API error: {e}"); return; }
        };
        let stop = resp.stop_reason.as_deref() != Some("tool_use");
        messages.push(assistant_message(&resp.content));
        if stop { return; }

        let mut results = Vec::new();
        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let output = match name.as_str() {
                    "bash" => harness::tools::run_bash(input["command"].as_str().unwrap_or(""), workdir),
                    "read_file" => harness::tools::run_read(input["path"].as_str().unwrap_or(""), workdir, input["limit"].as_u64().map(|l| l as usize)),
                    "write_file" => harness::tools::run_write(input["path"].as_str().unwrap_or(""), workdir, input["content"].as_str().unwrap_or("")),
                    "edit_file" => harness::tools::run_edit(input["path"].as_str().unwrap_or(""), workdir, input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or("")),
                    "background_run" => bg.run(
                        input["command"].as_str().unwrap_or(""),
                        workdir.to_path_buf(),
                        300,
                    ),
                    "check_background" => bg.check(input["task_id"].as_str()),
                    _ => format!("Unknown tool: {name}"),
                };
                println!("> {name}:");
                println!("{}", &output[..output.len().min(200)]);
                results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: output,
                });
            }
        }
        messages.push(tool_result_message(results));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load()?;
    let client = AnthropicClient::new(&config.api_key, &config.base_url, &config.model_id);
    let workdir = config.workdir.clone();
    let bg = BackgroundManager::new();
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms08 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir, &bg).await;

        if let Some(last) = history.last() {
            if let Some(content) = last["content"].as_array() {
                for block in content {
                    if let Some(text) = block["text"].as_str() { println!("{text}"); }
                }
            } else if let Some(text) = last["content"].as_str() { println!("{text}"); }
        }
        println!();
    }
    Ok(())
}
