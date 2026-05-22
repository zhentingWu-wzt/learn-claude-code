//! Harness: team mailboxes -- multiple models, coordinated through files.
//!
//! Persistent named agents with file-based JSONL inboxes. Each teammate runs
//! its own agent loop in a separate thread. Communication via append-only inboxes.

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef, MessageBus,
    assistant_message, tool_result_message, user_text, base_tools,
    messaging::VALID_MSG_TYPES};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

struct TeamConfig {
    team_name: String,
    members: Vec<serde_json::Value>,
}

struct TeammateManager {
    config: Mutex<TeamConfig>,
    bus: Arc<MessageBus>,
    config_path: std::path::PathBuf,
}

impl TeammateManager {
    fn new(team_dir: &std::path::Path, bus: Arc<MessageBus>) -> Self {
        std::fs::create_dir_all(team_dir).ok();
        let config_path = team_dir.join("config.json");
        let config = if config_path.exists() {
            let text = std::fs::read_to_string(&config_path).unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            TeamConfig {
                team_name: v["team_name"].as_str().unwrap_or("default").to_string(),
                members: v["members"].as_array().cloned().unwrap_or_default(),
            }
        } else {
            TeamConfig { team_name: "default".to_string(), members: Vec::new() }
        };
        Self { config: Mutex::new(config), bus, config_path }
    }

    fn save_config(&self) {
        let config = self.config.lock().unwrap();
        let json = serde_json::json!({
            "team_name": &config.team_name,
            "members": &config.members
        });
        std::fs::write(&self.config_path, serde_json::to_string_pretty(&json).unwrap_or_default()).ok();
    }

    fn spawn(&self, name: &str, role: &str, _prompt: &str) -> String {
        let mut config = self.config.lock().unwrap();
        if let Some(member) = config.members.iter_mut().find(|m| m["name"].as_str() == Some(name)) {
            let status = member["status"].as_str().unwrap_or("idle");
            if status != "idle" && status != "shutdown" {
                return format!("Error: '{name}' is currently {status}");
            }
            member["status"] = serde_json::Value::String("working".to_string());
            member["role"] = serde_json::Value::String(role.to_string());
        } else {
            config.members.push(serde_json::json!({"name": name, "role": role, "status": "working"}));
        }
        drop(config);
        self.save_config();
        format!("Spawned '{name}' (role: {role})")
    }

    fn set_status(&self, name: &str, status: &str) {
        let mut config = self.config.lock().unwrap();
        if let Some(member) = config.members.iter_mut().find(|m| m["name"].as_str() == Some(name)) {
            member["status"] = serde_json::Value::String(status.to_string());
        }
        drop(config);
        self.save_config();
    }

    fn list_all(&self) -> String {
        let config = self.config.lock().unwrap();
        if config.members.is_empty() { return "No teammates.".to_string(); }
        let mut lines = vec![format!("Team: {}", config.team_name)];
        for m in &config.members {
            let name = m["name"].as_str().unwrap_or("?");
            let role = m["role"].as_str().unwrap_or("?");
            let status = m["status"].as_str().unwrap_or("?");
            lines.push(format!("  {name} ({role}): {status}"));
        }
        lines.join("\n")
    }

    fn member_names(&self) -> Vec<String> {
        let config = self.config.lock().unwrap();
        config.members.iter()
            .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
            .collect()
    }
}

fn team_tools() -> Vec<ToolDef> {
    let mut tools = base_tools();
    tools.push(ToolDef {
        name: "spawn_teammate".to_string(),
        description: "Spawn a persistent teammate that runs in its own thread.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}, "role": {"type": "string"}, "prompt": {"type": "string"}},
            "required": ["name", "role", "prompt"]
        }),
    });
    tools.push(ToolDef {
        name: "list_teammates".to_string(),
        description: "List all teammates with name, role, status.".to_string(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}),
    });
    tools.push(ToolDef {
        name: "send_message".to_string(),
        description: "Send a message to a teammate's inbox.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "to": {"type": "string"},
                "content": {"type": "string"},
                "msg_type": {"type": "string", "enum": VALID_MSG_TYPES}
            },
            "required": ["to", "content"]
        }),
    });
    tools.push(ToolDef {
        name: "read_inbox".to_string(),
        description: "Read and drain the lead's inbox.".to_string(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}),
    });
    tools.push(ToolDef {
        name: "broadcast".to_string(),
        description: "Send a message to all teammates.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"content": {"type": "string"}},
            "required": ["content"]
        }),
    });
    tools
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &std::path::Path,
    bus: &MessageBus,
    team: &TeammateManager,
) {
    let system = format!("You are a team lead at {}. Spawn teammates and communicate via inboxes.", workdir.display());
    let tools = team_tools();

    loop {
        let inbox = bus.read_inbox("lead");
        if !inbox.is_empty() {
            messages.push(user_text(&format!("<inbox>{}</inbox>", serde_json::to_string_pretty(&inbox).unwrap_or_default())));
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
                    "spawn_teammate" => team.spawn(
                        input["name"].as_str().unwrap_or(""),
                        input["role"].as_str().unwrap_or(""),
                        input["prompt"].as_str().unwrap_or(""),
                    ),
                    "list_teammates" => team.list_all(),
                    "send_message" => bus.send(
                        "lead",
                        input["to"].as_str().unwrap_or(""),
                        input["content"].as_str().unwrap_or(""),
                        input["msg_type"].as_str().unwrap_or("message"),
                        None,
                    ),
                    "read_inbox" => serde_json::to_string_pretty(&bus.read_inbox("lead")).unwrap_or_default(),
                    "broadcast" => bus.broadcast("lead", input["content"].as_str().unwrap_or(""), &team.member_names()),
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
    let team_dir = workdir.join(".team");
    let inbox_dir = team_dir.join("inbox");
    let bus = Arc::new(MessageBus::new(&inbox_dir));
    let team = TeammateManager::new(&team_dir, bus.clone());
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms09 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        if query == "/team" { println!("{}", team.list_all()); continue; }
        if query == "/inbox" { println!("{}", serde_json::to_string_pretty(&bus.read_inbox("lead")).unwrap_or_default()); continue; }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir, &bus, &team).await;

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
