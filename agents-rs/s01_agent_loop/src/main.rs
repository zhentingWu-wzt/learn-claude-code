//! Harness: the loop -- the model's first connection to the real world.
//!
//! The entire secret of an AI coding agent in one pattern:
//!
//!     while stop_reason == "tool_use":
//!         response = LLM(messages, tools)
//!         execute tools
//!         append results

use anyhow::Result;
use harness::{ AnthropicClient, Config, ContentBlock, ToolDef, assistant_message,
    tool_result_message, user_text};
use std::io::{self, Write};

fn bash_tool() -> ToolDef {
    ToolDef {
        name: "bash".to_string(),
        description: "Run a shell command.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"]
        }),
    }
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &std::path::Path,
) {
    let system = format!("You are a coding agent at {}. Use bash to solve tasks. Act, don't explain.", workdir.display());
    let tools = vec![bash_tool()];

    loop {
        let resp = match client.create_message(&system, messages, &tools, 8000).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("API error: {e}");
                return;
            }
        };
        let stop = resp.stop_reason.as_deref() != Some("tool_use");
        messages.push(assistant_message(&resp.content));
        if stop {
            return;
        }
        let mut results = Vec::new();
        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let output = if name == "bash" {
                    let cmd = input["command"].as_str().unwrap_or("");
                    println!("\x1b[33m$ {cmd}\x1b[0m");
                    harness::tools::run_bash(cmd, workdir)
                } else {
                    format!("Unknown tool: {name}")
                };
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
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms01 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" {
            break;
        }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir).await;

        // Print final text response
        if let Some(last) = history.last() {
            if let Some(content) = last["content"].as_array() {
                for block in content {
                    if let Some(text) = block["text"].as_str() {
                        println!("{text}");
                    }
                }
            } else if let Some(text) = last["content"].as_str() {
                println!("{text}");
            }
        }
        println!();
    }
    Ok(())
}
