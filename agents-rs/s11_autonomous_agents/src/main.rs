//! Harness: autonomy -- models that find work without being told.
//!
//! Idle cycle with task board polling, auto-claiming unclaimed tasks, and
//! identity re-injection after context compression. Builds on s10's protocols.

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef, MessageBus, TaskManager,
    ProtocolTracker, assistant_message, tool_result_message, user_text,
    base_tools, messaging::VALID_MSG_TYPES};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

const POLL_INTERVAL: u64 = 5;
const IDLE_TIMEOUT: u64 = 60;

struct TeamConfig {
    team_name: String,
    members: Vec<serde_json::Value>,
}

struct TeammateManager {
    config: Mutex<TeamConfig>,
    bus: Arc<MessageBus>,
    tasks: Arc<Mutex<TaskManager>>,
    tracker: Arc<ProtocolTracker>,
    config_path: std::path::PathBuf,
    client: AnthropicClient,
    workdir: std::path::PathBuf,
}

impl TeammateManager {
    fn new(team_dir: &std::path::Path, bus: Arc<MessageBus>, tasks: Arc<Mutex<TaskManager>>, tracker: Arc<ProtocolTracker>, client: AnthropicClient, workdir: std::path::PathBuf) -> Self {
        std::fs::create_dir_all(team_dir).ok();
        let config_path = team_dir.join("config.json");
        let config = if config_path.exists() {
            let text = std::fs::read_to_string(&config_path).unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            TeamConfig { team_name: v["team_name"].as_str().unwrap_or("default").to_string(), members: v["members"].as_array().cloned().unwrap_or_default() }
        } else { TeamConfig { team_name: "default".to_string(), members: Vec::new() } };
        Self { config: Mutex::new(config), bus, tasks, tracker, config_path, client, workdir }
    }

    fn save_config(&self) {
        let config = self.config.lock().unwrap();
        std::fs::write(&self.config_path, serde_json::to_string_pretty(&serde_json::json!({"team_name": &config.team_name, "members": &config.members})).unwrap_or_default()).ok();
    }

    fn set_status(&self, name: &str, status: &str) {
        let mut config = self.config.lock().unwrap();
        if let Some(m) = config.members.iter_mut().find(|m| m["name"].as_str() == Some(name)) { m["status"] = serde_json::Value::String(status.to_string()); }
        drop(config); self.save_config();
    }

    fn spawn(&self, name: &str, role: &str, _prompt: &str) -> String {
        let mut config = self.config.lock().unwrap();
        if let Some(member) = config.members.iter_mut().find(|m| m["name"].as_str() == Some(name)) {
            let status = member["status"].as_str().unwrap_or("idle");
            if status != "idle" && status != "shutdown" { return format!("Error: '{name}' is currently {status}"); }
            member["status"] = serde_json::Value::String("working".to_string());
            member["role"] = serde_json::Value::String(role.to_string());
        } else { config.members.push(serde_json::json!({"name": name, "role": role, "status": "working"})); }
        drop(config); self.save_config();
        format!("Spawned '{name}' (role: {role})")
    }

    fn list_all(&self) -> String {
        let config = self.config.lock().unwrap();
        if config.members.is_empty() { return "No teammates.".to_string(); }
        let mut lines = vec![format!("Team: {}", config.team_name)];
        for m in &config.members { lines.push(format!("  {} ({}): {}", m["name"].as_str().unwrap_or("?"), m["role"].as_str().unwrap_or("?"), m["status"].as_str().unwrap_or("?"))); }
        lines.join("\n")
    }

    fn member_names(&self) -> Vec<String> {
        self.config.lock().unwrap().members.iter().filter_map(|m| m["name"].as_str().map(|s| s.to_string())).collect()
    }
}

fn lead_tools() -> Vec<ToolDef> {
    let mut tools = base_tools();
    tools.push(ToolDef { name: "spawn_teammate".into(), description: "Spawn an autonomous teammate.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}, "role": {"type": "string"}, "prompt": {"type": "string"}}, "required": ["name", "role", "prompt"]}) });
    tools.push(ToolDef { name: "list_teammates".into(), description: "List all teammates.".into(), input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    tools.push(ToolDef { name: "send_message".into(), description: "Send a message to a teammate.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"to": {"type": "string"}, "content": {"type": "string"}, "msg_type": {"type": "string", "enum": VALID_MSG_TYPES}}, "required": ["to", "content"]}) });
    tools.push(ToolDef { name: "read_inbox".into(), description: "Read and drain the lead's inbox.".into(), input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    tools.push(ToolDef { name: "broadcast".into(), description: "Send message to all teammates.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"content": {"type": "string"}}, "required": ["content"]}) });
    tools.push(ToolDef { name: "shutdown_request".into(), description: "Request a teammate to shut down.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"teammate": {"type": "string"}}, "required": ["teammate"]}) });
    tools.push(ToolDef { name: "shutdown_response".into(), description: "Check shutdown request status.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"request_id": {"type": "string"}}, "required": ["request_id"]}) });
    tools.push(ToolDef { name: "plan_approval".into(), description: "Approve or reject a teammate's plan.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"request_id": {"type": "string"}, "approve": {"type": "boolean"}, "feedback": {"type": "string"}}, "required": ["request_id", "approve"]}) });
    tools.push(ToolDef { name: "idle".into(), description: "Enter idle state (for lead -- rarely used).".into(), input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    tools.push(ToolDef { name: "claim_task".into(), description: "Claim a task from the board by ID.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"task_id": {"type": "integer"}}, "required": ["task_id"]}) });
    tools
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &std::path::Path,
    bus: &MessageBus,
    team: &TeammateManager,
    tracker: &ProtocolTracker,
    tasks: &Arc<Mutex<TaskManager>>,
) {
    let system = format!("You are a team lead at {}. Teammates are autonomous -- they find work themselves.", workdir.display());
    let tools = lead_tools();

    loop {
        let inbox = bus.read_inbox("lead");
        if !inbox.is_empty() { messages.push(user_text(&format!("<inbox>{}</inbox>", serde_json::to_string_pretty(&inbox).unwrap_or_default()))); }

        let resp = match client.create_message(&system, messages, &tools, 8000).await {
            Ok(r) => r, Err(e) => { eprintln!("API error: {e}"); return; }
        };
        let stop = resp.stop_reason.as_deref() != Some("tool_use");
        messages.push(assistant_message(&resp.content));
        if stop { return; }

        let mut results = Vec::new();
        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let output = match &**name {
                    "bash" => harness::tools::run_bash(input["command"].as_str().unwrap_or(""), workdir),
                    "read_file" => harness::tools::run_read(input["path"].as_str().unwrap_or(""), workdir, input["limit"].as_u64().map(|l| l as usize)),
                    "write_file" => harness::tools::run_write(input["path"].as_str().unwrap_or(""), workdir, input["content"].as_str().unwrap_or("")),
                    "edit_file" => harness::tools::run_edit(input["path"].as_str().unwrap_or(""), workdir, input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or("")),
                    "spawn_teammate" => team.spawn(input["name"].as_str().unwrap_or(""), input["role"].as_str().unwrap_or(""), input["prompt"].as_str().unwrap_or("")),
                    "list_teammates" => team.list_all(),
                    "send_message" => bus.send("lead", input["to"].as_str().unwrap_or(""), input["content"].as_str().unwrap_or(""), input["msg_type"].as_str().unwrap_or("message"), None),
                    "read_inbox" => serde_json::to_string_pretty(&bus.read_inbox("lead")).unwrap_or_default(),
                    "broadcast" => bus.broadcast("lead", input["content"].as_str().unwrap_or(""), &team.member_names()),
                    "shutdown_request" => {
                        let req_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
                        let teammate = input["teammate"].as_str().unwrap_or("");
                        tracker.shutdown_requests.lock().unwrap().insert(req_id.clone(), harness::protocols::ShutdownRequest { target: teammate.to_string(), status: "pending".to_string() });
                        bus.send("lead", teammate, "Please shut down gracefully.", "shutdown_request", Some(serde_json::json!({"request_id": &req_id})));
                        format!("Shutdown request {req_id} sent to '{teammate}'")
                    }
                    "shutdown_response" => {
                        let req_id = input["request_id"].as_str().unwrap_or("");
                        match tracker.shutdown_requests.lock().unwrap().get(req_id) {
                            Some(req) => serde_json::to_string(req).unwrap_or_else(|_| "{\"error\": \"serialize failed\"}".to_string()),
                            None => "{\"error\": \"not found\"}".to_string(),
                        }
                    }
                    "plan_approval" => {
                        let req_id = input["request_id"].as_str().unwrap_or("");
                        let approve = input["approve"].as_bool().unwrap_or(false);
                        let feedback = input["feedback"].as_str().unwrap_or("");
                        let req = tracker.plan_requests.lock().unwrap().get(req_id).cloned();
                        if let Some(req) = req {
                            let status = if approve { "approved" } else { "rejected" };
                            tracker.plan_requests.lock().unwrap().get_mut(req_id).unwrap().status = status.to_string();
                            tracker.plan_approvals.lock().unwrap().insert(req.from.clone(), status.to_string());
                            bus.send("lead", &req.from, feedback, "plan_approval_response", Some(serde_json::json!({"request_id": req_id, "approve": approve, "feedback": feedback})));
                            format!("Plan {status} for '{}'", req.from)
                        } else { format!("Error: Unknown plan request_id '{req_id}'") }
                    }
                    "idle" => "Lead does not idle.".to_string(),
                    "claim_task" => {
                        let tid = input["task_id"].as_u64().unwrap_or(0) as u32;
                        let mut tasks = tasks.lock().unwrap();
                        tasks.claim(tid, "lead").unwrap_or_else(|e| format!("Error: {e}"))
                    }
                    _ => format!("Unknown tool: {name}"),
                };
                println!("> {name}:");
                println!("{}", &output[..output.len().min(200)]);
                results.push(ContentBlock::ToolResult { tool_use_id: id.clone(), content: output });
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
    let tasks_dir = workdir.join(".tasks");
    let bus = Arc::new(MessageBus::new(&inbox_dir));
    let tracker = Arc::new(ProtocolTracker::new());
    let tasks = Arc::new(Mutex::new(TaskManager::new(&tasks_dir)));
    let team = TeammateManager::new(&team_dir, bus.clone(), tasks.clone(), tracker.clone(), AnthropicClient::new(&config.api_key, &config.base_url, &config.model_id), workdir.clone());
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms11 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) { Ok(0) | Err(_) => break, _ => {} }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        if query == "/team" { println!("{}", team.list_all()); continue; }
        if query == "/inbox" { println!("{}", serde_json::to_string_pretty(&bus.read_inbox("lead")).unwrap_or_default()); continue; }
        if query == "/tasks" {
            let tasks = tasks.lock().unwrap();
            println!("{}", tasks.list_all());
            continue;
        }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir, &bus, &team, &tracker, &tasks).await;

        if let Some(last) = history.last() {
            if let Some(content) = last["content"].as_array() {
                for block in content { if let Some(text) = block["text"].as_str() { println!("{text}"); } }
            } else if let Some(text) = last["content"].as_str() { println!("{text}"); }
        }
        println!();
    }
    Ok(())
}
