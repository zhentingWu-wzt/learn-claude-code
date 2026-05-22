//! Harness: all mechanisms combined -- the complete cockpit for the model.
//!
//! Capstone implementation combining every mechanism from s01-s11.
//! Session s12 (task-aware worktree isolation) is taught separately.
//! NOT a teaching session -- this is the "put it all together" reference.
//!
//! REPL commands: /compact /tasks /team /inbox

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef, MessageBus, TaskManager,
    TodoManager, SkillLoader, ProtocolTracker,
    assistant_message, tool_result_message, user_text, base_tools, extract_text,
    micro_compact, auto_compact, estimate_tokens,
    messaging::VALID_MSG_TYPES};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

const TOKEN_THRESHOLD: usize = 100000;

fn full_tools() -> Vec<ToolDef> {
    let mut tools = base_tools();
    // s03: TodoWrite
    tools.push(ToolDef { name: "TodoWrite".into(), description: "Update task tracking list.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"items": {"type": "array", "items": {"type": "object", "properties": {"content": {"type": "string"}, "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}, "activeForm": {"type": "string"}}, "required": ["content", "status", "activeForm"]}}}, "required": ["items"]}) });
    // s04: task (subagent)
    tools.push(ToolDef { name: "task".into(), description: "Spawn a subagent for isolated exploration or work.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"prompt": {"type": "string"}, "agent_type": {"type": "string", "enum": ["Explore", "general-purpose"]}}, "required": ["prompt"]}) });
    // s05: load_skill
    tools.push(ToolDef { name: "load_skill".into(), description: "Load specialized knowledge by name.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}) });
    // s06: compress
    tools.push(ToolDef { name: "compress".into(), description: "Manually compress conversation context.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    // s08: background
    tools.push(ToolDef { name: "background_run".into(), description: "Run command in background thread.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"command": {"type": "string"}, "timeout": {"type": "integer"}}, "required": ["command"]}) });
    tools.push(ToolDef { name: "check_background".into(), description: "Check background task status.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"task_id": {"type": "string"}}}) });
    // s07: file tasks
    tools.push(ToolDef { name: "task_create".into(), description: "Create a persistent file task.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"subject": {"type": "string"}, "description": {"type": "string"}}, "required": ["subject"]}) });
    tools.push(ToolDef { name: "task_get".into(), description: "Get task details by ID.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"task_id": {"type": "integer"}}, "required": ["task_id"]}) });
    tools.push(ToolDef { name: "task_update".into(), description: "Update task status or dependencies.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"task_id": {"type": "integer"}, "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "deleted"]}, "add_blocked_by": {"type": "array", "items": {"type": "integer"}}, "remove_blocked_by": {"type": "array", "items": {"type": "integer"}}}, "required": ["task_id"]}) });
    tools.push(ToolDef { name: "task_list".into(), description: "List all tasks.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    // s09: team
    tools.push(ToolDef { name: "spawn_teammate".into(), description: "Spawn a persistent autonomous teammate.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}, "role": {"type": "string"}, "prompt": {"type": "string"}}, "required": ["name", "role", "prompt"]}) });
    tools.push(ToolDef { name: "list_teammates".into(), description: "List all teammates.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    tools.push(ToolDef { name: "send_message".into(), description: "Send a message to a teammate.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"to": {"type": "string"}, "content": {"type": "string"}, "msg_type": {"type": "string", "enum": VALID_MSG_TYPES}}, "required": ["to", "content"]}) });
    tools.push(ToolDef { name: "read_inbox".into(), description: "Read and drain the lead's inbox.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    tools.push(ToolDef { name: "broadcast".into(), description: "Send message to all teammates.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"content": {"type": "string"}}, "required": ["content"]}) });
    // s10: protocols
    tools.push(ToolDef { name: "shutdown_request".into(), description: "Request a teammate to shut down.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"teammate": {"type": "string"}}, "required": ["teammate"]}) });
    tools.push(ToolDef { name: "plan_approval".into(), description: "Approve or reject a teammate's plan.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"request_id": {"type": "string"}, "approve": {"type": "boolean"}, "feedback": {"type": "string"}}, "required": ["request_id", "approve"]}) });
    // s11: idle + claim
    tools.push(ToolDef { name: "idle".into(), description: "Enter idle state.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    tools.push(ToolDef { name: "claim_task".into(), description: "Claim a task from the board.".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"task_id": {"type": "integer"}}, "required": ["task_id"]}) });
    tools
}

async fn run_subagent(client: &AnthropicClient, prompt: &str, workdir: &std::path::Path) -> String {
    let sub_tools = base_tools();
    let mut sub_msgs = vec![user_text(prompt)];
    let mut last_resp = None;
    for _ in 0..30 {
        let resp = match client.create_message("You are a coding subagent. Complete the task and summarize.", &sub_msgs, &sub_tools, 8000).await {
            Ok(r) => r, Err(_) => break,
        };
        let stop = resp.stop_reason.as_deref() != Some("tool_use");
        sub_msgs.push(assistant_message(&resp.content));
        last_resp = Some(resp.clone());
        if stop { break; }
        let mut results = Vec::new();
        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let output = match name.as_str() {
                    "bash" => harness::tools::run_bash(input["command"].as_str().unwrap_or(""), workdir),
                    "read_file" => harness::tools::run_read(input["path"].as_str().unwrap_or(""), workdir, None),
                    "write_file" => harness::tools::run_write(input["path"].as_str().unwrap_or(""), workdir, input["content"].as_str().unwrap_or("")),
                    "edit_file" => harness::tools::run_edit(input["path"].as_str().unwrap_or(""), workdir, input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or("")),
                    _ => format!("Unknown tool: {name}"),
                };
                results.push(ContentBlock::ToolResult { tool_use_id: id.clone(), content: output });
            }
        }
        if !results.is_empty() { sub_msgs.push(tool_result_message(results)); }
    }
    if let Some(r) = &last_resp {
        let text = extract_text(&r.content);
        if text.is_empty() { "(no summary)".to_string() } else { text }
    } else { "(subagent failed)".to_string() }
}

struct App {
    client: AnthropicClient,
    workdir: std::path::PathBuf,
    todo: Arc<Mutex<TodoManager>>,
    skills: SkillLoader,
    tasks: Arc<Mutex<TaskManager>>,
    bus: Arc<MessageBus>,
    tracker: Arc<ProtocolTracker>,
    transcript_dir: std::path::PathBuf,
}

impl App {
    async fn agent_loop(&self, messages: &mut Vec<serde_json::Value>) {
        let system = format!(
            "You are a coding agent at {}. Use tools to solve tasks.\n\
             Prefer task_create/task_update/task_list for multi-step work. Use TodoWrite for short checklists.\n\
             Use task for subagent delegation. Use load_skill for specialized knowledge.\n\
             Skills: {}",
            self.workdir.display(),
            self.skills.get_descriptions()
        );
        let tools = full_tools();
        let mut rounds_without_todo: usize = 0;

        loop {
            // s06: compression
            {
                let msgs_mut: &mut [serde_json::Value] = messages.as_mut_slice();
                micro_compact(msgs_mut);
            }
            if estimate_tokens(messages) > TOKEN_THRESHOLD {
                println!("[auto-compact triggered]");
                match auto_compact(&self.client, messages, &self.transcript_dir).await {
                    Ok(compressed) => *messages = compressed,
                    Err(e) => eprintln!("auto_compact error: {e}"),
                }
            }

            // s09: check inbox
            let inbox = self.bus.read_inbox("lead");
            if !inbox.is_empty() {
                messages.push(user_text(&format!("<inbox>{}</inbox>", serde_json::to_string_pretty(&inbox).unwrap_or_default())));
            }

            let resp = match self.client.create_message(&system, messages, &tools, 8000).await {
                Ok(r) => r, Err(e) => { eprintln!("API error: {e}"); return; }
            };
            let stop = resp.stop_reason.as_deref() != Some("tool_use");
            messages.push(assistant_message(&resp.content));
            if stop { return; }

            let mut results = Vec::new();
            let mut used_todo = false;
            let mut manual_compress = false;

            for block in &resp.content {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    if name == "compress" { manual_compress = true; }
                    let output = self.dispatch_tool(name, input).await;
                    if name == "TodoWrite" { used_todo = true; }
                    println!("> {name}:");
                    println!("{}", &output[..output.len().min(200)]);
                    results.push(ContentBlock::ToolResult { tool_use_id: id.clone(), content: output });
                }
            }

            // s03: nag reminder
            rounds_without_todo = if used_todo { 0 } else { rounds_without_todo + 1 };
            let todo = self.todo.lock().unwrap();
            if todo.has_open_items() && rounds_without_todo >= 3 {
                results.push(ContentBlock::Text { text: "<reminder>Update your todos.</reminder>".to_string() });
            }
            drop(todo);

            messages.push(tool_result_message(results));

            // s06: manual compress
            if manual_compress {
                println!("[manual compact]");
                match auto_compact(&self.client, messages, &self.transcript_dir).await {
                    Ok(compressed) => *messages = compressed,
                    Err(e) => eprintln!("compact error: {e}"),
                }
                return;
            }
        }
    }

    async fn dispatch_tool(&self, name: &str, input: &serde_json::Value) -> String {
        match name {
            "bash" => harness::tools::run_bash(input["command"].as_str().unwrap_or(""), &self.workdir),
            "read_file" => harness::tools::run_read(input["path"].as_str().unwrap_or(""), &self.workdir, input["limit"].as_u64().map(|l| l as usize)),
            "write_file" => harness::tools::run_write(input["path"].as_str().unwrap_or(""), &self.workdir, input["content"].as_str().unwrap_or("")),
            "edit_file" => harness::tools::run_edit(input["path"].as_str().unwrap_or(""), &self.workdir, input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or("")),
            "TodoWrite" => {
                let items = input["items"].as_array().cloned().unwrap_or_default();
                let mut todo = self.todo.lock().unwrap();
                todo.update(items).unwrap_or_else(|e| format!("Error: {e}"))
            }
            "task" => run_subagent(&self.client, input["prompt"].as_str().unwrap_or(""), &self.workdir).await,
            "load_skill" => self.skills.get_content(input["name"].as_str().unwrap_or("")),
            "compress" => "Compressing...".to_string(),
            "background_run" => format!("Background run not yet implemented in s_full Rust"),
            "check_background" => "No background tasks.".to_string(),
            "task_create" => { let mut t = self.tasks.lock().unwrap(); t.create(input["subject"].as_str().unwrap_or(""), input["description"].as_str().unwrap_or("")).unwrap_or_else(|e| format!("Error: {e}")) }
            "task_get" => { let t = self.tasks.lock().unwrap(); t.get(input["task_id"].as_u64().unwrap_or(0) as u32).unwrap_or_else(|e| format!("Error: {e}")) }
            "task_update" => {
                let mut t = self.tasks.lock().unwrap();
                let tid = input["task_id"].as_u64().unwrap_or(0) as u32;
                let status = input["status"].as_str();
                let add: Option<Vec<u32>> = input["add_blocked_by"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|n| n as u32)).collect());
                let remove: Option<Vec<u32>> = input["remove_blocked_by"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|n| n as u32)).collect());
                t.update(tid, status, add, remove).unwrap_or_else(|e| format!("Error: {e}"))
            }
            "task_list" => self.tasks.lock().unwrap().list_all(),
            "spawn_teammate" => format!("Spawned teammate '{}' (teammate loop not yet implemented in Rust s_full)", input["name"].as_str().unwrap_or("?")),
            "list_teammates" => "No teammates.".to_string(),
            "send_message" => self.bus.send("lead", input["to"].as_str().unwrap_or(""), input["content"].as_str().unwrap_or(""), input["msg_type"].as_str().unwrap_or("message"), None),
            "read_inbox" => serde_json::to_string_pretty(&self.bus.read_inbox("lead")).unwrap_or_default(),
            "broadcast" => self.bus.broadcast("lead", input["content"].as_str().unwrap_or(""), &[]),
            "shutdown_request" => format!("Shutdown request sent (protocol not yet implemented in Rust s_full)"),
            "plan_approval" => format!("Plan review (protocol not yet implemented in Rust s_full)"),
            "idle" => "Lead does not idle.".to_string(),
            "claim_task" => { let mut t = self.tasks.lock().unwrap(); t.claim(input["task_id"].as_u64().unwrap_or(0) as u32, "lead").unwrap_or_else(|e| format!("Error: {e}")) }
            _ => format!("Unknown tool: {name}"),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load()?;
    let client = AnthropicClient::new(&config.api_key, &config.base_url, &config.model_id);
    let workdir = config.workdir.clone();

    let app = App {
        client,
        workdir: workdir.clone(),
        todo: Arc::new(Mutex::new(TodoManager::new())),
        skills: SkillLoader::new(&workdir.join("skills")),
        tasks: Arc::new(Mutex::new(TaskManager::new(&workdir.join(".tasks")))),
        bus: Arc::new(MessageBus::new(&workdir.join(".team").join("inbox"))),
        tracker: Arc::new(ProtocolTracker::new()),
        transcript_dir: workdir.join(".transcripts"),
    };

    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms_full >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) { Ok(0) | Err(_) => break, _ => {} }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        if query == "/compact" {
            if !history.is_empty() {
                println!("[manual compact via /compact]");
                match auto_compact(&app.client, &mut history, &app.transcript_dir).await {
                    Ok(compressed) => history = compressed,
                    Err(e) => eprintln!("compact error: {e}"),
                }
            }
            continue;
        }
        if query == "/tasks" { println!("{}", app.tasks.lock().unwrap().list_all()); continue; }
        if query == "/team" { println!("No teammates."); continue; }
        if query == "/inbox" { println!("{}", serde_json::to_string_pretty(&app.bus.read_inbox("lead")).unwrap_or_default()); continue; }

        history.push(user_text(query));
        app.agent_loop(&mut history).await;

        if let Some(last) = history.last() {
            if let Some(content) = last["content"].as_array() {
                for block in content { if let Some(text) = block["text"].as_str() { println!("{text}"); } }
            } else if let Some(text) = last["content"].as_str() { println!("{text}"); }
        }
        println!();
    }
    Ok(())
}
