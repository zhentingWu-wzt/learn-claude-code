//! Harness: planning -- keeping the model on course without scripting the route.
//!
//! The model tracks its own progress via a TodoManager. A nag reminder
//! forces it to keep updating when it forgets.

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef, TodoManager,
    assistant_message, tool_result_message, user_text, base_tools};
use std::io::{self, Write};
use std::sync::Mutex;

fn todo_tool() -> ToolDef {
    ToolDef {
        name: "todo".to_string(),
        description: "Update task list. Track progress on multi-step tasks.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string"},
                            "text": {"type": "string"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                        },
                        "required": ["id", "text", "status"]
                    }
                }
            },
            "required": ["items"]
        }),
    }
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &std::path::Path,
    todo: &Mutex<TodoManager>,
) {
    let system = format!(
        "You are a coding agent at {}.\nUse the todo tool to plan multi-step tasks. \
         Mark in_progress before starting, completed when done.\nPrefer tools over prose.",
        workdir.display()
    );
    let mut tools = base_tools();
    tools.push(todo_tool());

    let mut rounds_since_todo: usize = 0;

    loop {
        let resp = match client.create_message(&system, messages, &tools, 8000).await {
            Ok(r) => r,
            Err(e) => { eprintln!("API error: {e}"); return; }
        };
        let stop = resp.stop_reason.as_deref() != Some("tool_use");
        messages.push(assistant_message(&resp.content));
        if stop { return; }

        let mut results = Vec::new();
        let mut used_todo = false;

        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let output = match name.as_str() {
                    "bash" => harness::tools::run_bash(input["command"].as_str().unwrap_or(""), workdir),
                    "read_file" => harness::tools::run_read(input["path"].as_str().unwrap_or(""), workdir, input["limit"].as_u64().map(|l| l as usize)),
                    "write_file" => harness::tools::run_write(input["path"].as_str().unwrap_or(""), workdir, input["content"].as_str().unwrap_or("")),
                    "edit_file" => harness::tools::run_edit(input["path"].as_str().unwrap_or(""), workdir, input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or("")),
                    "todo" => {
                        used_todo = true;
                        let items = input["items"].as_array().cloned().unwrap_or_default();
                        let mut todo = todo.lock().unwrap();
                        todo.update(items).unwrap_or_else(|e| format!("Error: {e}"))
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

        rounds_since_todo = if used_todo { 0 } else { rounds_since_todo + 1 };
        if rounds_since_todo >= 3 {
            results.push(ContentBlock::Text {
                text: "<reminder>Update your todos.</reminder>".to_string(),
            });
        }

        messages.push(tool_result_message(results));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load()?;
    let client = AnthropicClient::new(&config.api_key, &config.base_url, &config.model_id);
    let workdir = config.workdir.clone();
    let todo = Mutex::new(TodoManager::new());
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms03 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir, &todo).await;

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
