//! Harness: context isolation -- protecting the model's clarity of thought.
//!
//! Spawn a child agent with fresh messages=[]. The child works in its own
//! context, sharing the filesystem, then returns only a summary to the parent.

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef,
    assistant_message, tool_result_message, user_text, base_tools, extract_text};
use std::io::{self, Write};
use std::path::Path;

fn task_tool() -> ToolDef {
    ToolDef {
        name: "task".to_string(),
        description: "Spawn a subagent with fresh context. It shares the filesystem but not conversation history.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"prompt": {"type": "string"}, "description": {"type": "string"}},
            "required": ["prompt"]
        }),
    }
}

async fn run_subagent(client: &AnthropicClient, prompt: &str, workdir: &Path) -> String {
    let sub_tools = base_tools();
    let mut sub_msgs = vec![user_text(prompt)];
    let mut last_resp = None;

    for _ in 0..30 {
        let resp = match client.create_message(
            "You are a coding subagent. Complete the task and summarize your findings.",
            &sub_msgs, &sub_tools, 8000,
        ).await {
            Ok(r) => r,
            Err(_) => break,
        };
        let stop = resp.stop_reason.as_deref() != Some("tool_use");
        sub_msgs.push(assistant_message(&resp.content));
        last_resp = Some(resp.clone());
        if stop { break; }

        let mut results: Vec<ContentBlock> = Vec::new();
        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let output = match &**name {
                    "bash" => harness::tools::run_bash(input["command"].as_str().unwrap_or(""), workdir),
                    "read_file" => harness::tools::run_read(input["path"].as_str().unwrap_or(""), workdir, input["limit"].as_u64().map(|l| l as usize)),
                    "write_file" => harness::tools::run_write(input["path"].as_str().unwrap_or(""), workdir, input["content"].as_str().unwrap_or("")),
                    "edit_file" => harness::tools::run_edit(input["path"].as_str().unwrap_or(""), workdir, input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or("")),
                    _ => format!("Unknown tool: {name}"),
                };
                results.push(ContentBlock::ToolResult { tool_use_id: id.clone(), content: output });
            }
        }
        if !results.is_empty() {
            sub_msgs.push(tool_result_message(results));
        }
    }

    if let Some(r) = &last_resp {
        let text = extract_text(&r.content);
        if text.is_empty() { "(no summary)".to_string() } else { text }
    } else {
        "(subagent failed)".to_string()
    }
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &Path,
) {
    let system = format!("You are a coding agent at {}. Use the task tool to delegate exploration or subtasks.", workdir.display());
    let mut tools = base_tools();
    tools.push(task_tool());

    loop {
        let resp = match client.create_message(&system, messages, &tools, 8000).await {
            Ok(r) => r,
            Err(e) => { eprintln!("API error: {e}"); return; }
        };
        let stop = resp.stop_reason.as_deref() != Some("tool_use");
        messages.push(assistant_message(&resp.content));
        if stop { return; }

        let mut results: Vec<ContentBlock> = Vec::new();
        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let output = if name == "task" {
                    let desc = input["description"].as_str().unwrap_or("subtask");
                    let prompt = input["prompt"].as_str().unwrap_or("");
                    println!("> task ({desc}): {}", &prompt[..prompt.len().min(80)]);
                    run_subagent(client, prompt, workdir).await
                } else {
                    match &**name {
                        "bash" => harness::tools::run_bash(input["command"].as_str().unwrap_or(""), workdir),
                        "read_file" => harness::tools::run_read(input["path"].as_str().unwrap_or(""), workdir, input["limit"].as_u64().map(|l| l as usize)),
                        "write_file" => harness::tools::run_write(input["path"].as_str().unwrap_or(""), workdir, input["content"].as_str().unwrap_or("")),
                        "edit_file" => harness::tools::run_edit(input["path"].as_str().unwrap_or(""), workdir, input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or("")),
                        _ => format!("Unknown tool: {name}"),
                    }
                };
                println!("  {}", &output[..output.len().min(200)]);
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
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms04 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) { Ok(0) | Err(_) => break, _ => {} }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir).await;

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
