//! Harness: compression -- clean memory for infinite sessions.
//!
//! Three-layer compression pipeline:
//! Layer 1: micro_compact (every turn, silent)
//! Layer 2: auto_compact (when tokens > threshold)
//! Layer 3: compact tool (manual trigger)

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef,
    assistant_message, tool_result_message, user_text, base_tools,
    micro_compact, auto_compact, estimate_tokens};
use std::io::{self, Write};

const THRESHOLD: usize = 50000;

fn compact_tool() -> ToolDef {
    ToolDef {
        name: "compact".to_string(),
        description: "Trigger manual conversation compression.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"focus": {"type": "string", "description": "What to preserve in the summary"}}
        }),
    }
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &std::path::Path,
    transcript_dir: &std::path::Path,
) {
    let system = format!("You are a coding agent at {}. Use tools to solve tasks.", workdir.display());
    let mut tools = base_tools();
    tools.push(compact_tool());

    loop {
        // Layer 1: micro_compact before each LLM call
        {
            let msgs_mut: &mut [serde_json::Value] = messages.as_mut_slice();
            micro_compact(msgs_mut);
        }

        // Layer 2: auto_compact if token estimate exceeds threshold
        if estimate_tokens(messages) > THRESHOLD {
            println!("[auto_compact triggered]");
            match auto_compact(client, messages, transcript_dir).await {
                Ok(compressed) => *messages = compressed,
                Err(e) => eprintln!("auto_compact error: {e}"),
            }
        }

        let resp = match client.create_message(&system, messages, &tools, 8000).await {
            Ok(r) => r,
            Err(e) => { eprintln!("API error: {e}"); return; }
        };
        let stop = resp.stop_reason.as_deref() != Some("tool_use");
        messages.push(assistant_message(&resp.content));
        if stop { return; }

        let mut results = Vec::new();
        let mut manual_compact = false;

        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let output = if name == "compact" {
                    manual_compact = true;
                    "Compressing...".to_string()
                } else {
                    match name.as_str() {
                        "bash" => harness::tools::run_bash(input["command"].as_str().unwrap_or(""), workdir),
                        "read_file" => harness::tools::run_read(input["path"].as_str().unwrap_or(""), workdir, input["limit"].as_u64().map(|l| l as usize)),
                        "write_file" => harness::tools::run_write(input["path"].as_str().unwrap_or(""), workdir, input["content"].as_str().unwrap_or("")),
                        "edit_file" => harness::tools::run_edit(input["path"].as_str().unwrap_or(""), workdir, input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or("")),
                        _ => format!("Unknown tool: {name}"),
                    }
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

        // Layer 3: manual compact
        if manual_compact {
            println!("[manual compact]");
            match auto_compact(client, messages, transcript_dir).await {
                Ok(compressed) => *messages = compressed,
                Err(e) => eprintln!("compact error: {e}"),
            }
            return;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load()?;
    let client = AnthropicClient::new(&config.api_key, &config.base_url, &config.model_id);
    let workdir = config.workdir.clone();
    let transcript_dir = workdir.join(".transcripts");
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms06 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir, &transcript_dir).await;

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
