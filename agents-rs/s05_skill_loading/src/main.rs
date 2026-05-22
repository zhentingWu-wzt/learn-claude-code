//! Harness: on-demand knowledge -- domain expertise, loaded when the model asks.
//!
//! Two-layer skill injection: Layer 1 = skill names in system prompt,
//! Layer 2 = full skill body returned in tool_result on demand.

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef, SkillLoader,
    assistant_message, tool_result_message, user_text, base_tools};
use std::io::{self, Write};

fn load_skill_tool() -> ToolDef {
    ToolDef {
        name: "load_skill".to_string(),
        description: "Load specialized knowledge by name.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string", "description": "Skill name to load"}},
            "required": ["name"]
        }),
    }
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &std::path::Path,
    skills: &SkillLoader,
) {
    let system = format!(
        "You are a coding agent at {}.\nUse load_skill to access specialized knowledge before tackling unfamiliar topics.\n\nSkills available:\n{}",
        workdir.display(),
        skills.get_descriptions()
    );
    let mut tools = base_tools();
    tools.push(load_skill_tool());

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
                    "load_skill" => skills.get_content(input["name"].as_str().unwrap_or("")),
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
    let skills = SkillLoader::new(&workdir.join("skills"));
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms05 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir, &skills).await;

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
