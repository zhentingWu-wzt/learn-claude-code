//! Harness: persistent tasks -- goals that outlive any single conversation.
//!
//! Tasks persist as JSON files in .tasks/ so they survive context compression.
//! Each task has a dependency graph (blockedBy).

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef, TaskManager,
    assistant_message, tool_result_message, user_text, base_tools};
use std::io::{self, Write};
use std::sync::Mutex;

fn task_create_tool() -> ToolDef {
    ToolDef {
        name: "task_create".to_string(),
        description: "Create a new task.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"subject": {"type": "string"}, "description": {"type": "string"}},
            "required": ["subject"]
        }),
    }
}

fn task_update_tool() -> ToolDef {
    ToolDef {
        name: "task_update".to_string(),
        description: "Update a task's status or dependencies.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "integer"},
                "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                "addBlockedBy": {"type": "array", "items": {"type": "integer"}},
                "removeBlockedBy": {"type": "array", "items": {"type": "integer"}}
            },
            "required": ["task_id"]
        }),
    }
}

fn task_list_tool() -> ToolDef {
    ToolDef {
        name: "task_list".to_string(),
        description: "List all tasks with status summary.".to_string(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}),
    }
}

fn task_get_tool() -> ToolDef {
    ToolDef {
        name: "task_get".to_string(),
        description: "Get full details of a task by ID.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"task_id": {"type": "integer"}},
            "required": ["task_id"]
        }),
    }
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &std::path::Path,
    tasks: &Mutex<TaskManager>,
) {
    let system = format!("You are a coding agent at {}. Use task tools to plan and track work.", workdir.display());
    let mut tools = base_tools();
    tools.push(task_create_tool());
    tools.push(task_update_tool());
    tools.push(task_list_tool());
    tools.push(task_get_tool());

    loop {
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
                    "task_create" => {
                        let mut tasks = tasks.lock().unwrap();
                        tasks.create(
                            input["subject"].as_str().unwrap_or(""),
                            input["description"].as_str().unwrap_or(""),
                        ).unwrap_or_else(|e| format!("Error: {e}"))
                    }
                    "task_update" => {
                        let mut tasks = tasks.lock().unwrap();
                        let tid = input["task_id"].as_u64().unwrap_or(0) as u32;
                        let status = input["status"].as_str();
                        let add: Option<Vec<u32>> = input["addBlockedBy"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|n| n as u32)).collect());
                        let remove: Option<Vec<u32>> = input["removeBlockedBy"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|n| n as u32)).collect());
                        tasks.update(tid, status, add, remove).unwrap_or_else(|e| format!("Error: {e}"))
                    }
                    "task_list" => {
                        let tasks = tasks.lock().unwrap();
                        tasks.list_all()
                    }
                    "task_get" => {
                        let tasks = tasks.lock().unwrap();
                        let tid = input["task_id"].as_u64().unwrap_or(0) as u32;
                        tasks.get(tid).unwrap_or_else(|e| format!("Error: {e}"))
                    }
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
    let tasks = Mutex::new(TaskManager::new(&workdir.join(".tasks")));
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms07 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir, &tasks).await;

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
