use crate::client::{AnthropicClient, ContentBlock};
use serde_json::Value;
use std::path::Path;

const KEEP_RECENT: usize = 3;
const PRESERVE_RESULT_TOOLS: &[&str] = &["read_file"];

pub fn estimate_tokens(messages: &[Value]) -> usize {
    serde_json::to_string(messages).map(|s| s.len() / 4).unwrap_or(0)
}

/// Layer 1: Replace old tool_result content with placeholders.
pub fn micro_compact(messages: &mut [Value]) {
    let mut tool_results: Vec<(usize, usize)> = Vec::new();
    for (msg_idx, msg) in messages.iter().enumerate() {
        if msg["role"] == "user" {
            if let Some(content) = msg["content"].as_array() {
                for (part_idx, part) in content.iter().enumerate() {
                    if part["type"] == "tool_result" {
                        tool_results.push((msg_idx, part_idx));
                    }
                }
            }
        }
    }
    if tool_results.len() <= KEEP_RECENT {
        return;
    }
    // Build tool_name map from assistant messages
    let mut tool_name_map = std::collections::HashMap::new();
    for msg in messages.iter() {
        if msg["role"] == "assistant" {
            if let Some(content) = msg["content"].as_array() {
                for block in content {
                    if block["type"] == "tool_use" {
                        if let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) {
                            tool_name_map.insert(id.to_string(), name.to_string());
                        }
                    }
                }
            }
        }
    }
    // Clear old results
    let to_clear = &tool_results[..tool_results.len() - KEEP_RECENT];
    for &(msg_idx, part_idx) in to_clear {
        let content = &mut messages[msg_idx]["content"];
        if let Some(parts) = content.as_array_mut() {
            let part = &mut parts[part_idx];
            let content_str = part["content"].as_str().unwrap_or("");
            if content_str.len() <= 100 {
                continue;
            }
            let tool_id = part["tool_use_id"].as_str().unwrap_or("");
            let tool_name = tool_name_map.get(tool_id).map(|s| s.as_str()).unwrap_or("unknown");
            if PRESERVE_RESULT_TOOLS.contains(&tool_name) {
                continue;
            }
            part["content"] = Value::String(format!("[Previous: used {tool_name}]"));
        }
    }
}

/// Layer 2: Save transcript, ask LLM to summarize, replace messages.
pub async fn auto_compact(
    client: &AnthropicClient,
    messages: &[Value],
    transcript_dir: &Path,
) -> anyhow::Result<Vec<Value>> {
    std::fs::create_dir_all(transcript_dir)?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let transcript_path = transcript_dir.join(format!("transcript_{timestamp}.jsonl"));
    {
        let mut f = std::fs::File::create(&transcript_path)?;
        for msg in messages {
            use std::io::Write;
            writeln!(f, "{}", serde_json::to_string(msg)?)?;
        }
    }
    println!("[transcript saved: {}]", transcript_path.display());

    let conv_text = serde_json::to_string(&messages)?;
    let conv_text = if conv_text.len() > 80000 {
        &conv_text[conv_text.len() - 80000..]
    } else {
        &conv_text
    };

    let summary_prompt = format!(
        "Summarize this conversation for continuity. Include: \
         1) What was accomplished, 2) Current state, 3) Key decisions made. \
         Be concise but preserve critical details.\n\n{conv_text}"
    );
    let msgs = vec![serde_json::json!({"role": "user", "content": summary_prompt})];
    let resp = client.create_simple_message(&msgs, 2000).await?;
    let summary = resp.content.iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    Ok(vec![serde_json::json!({
        "role": "user",
        "content": format!("[Conversation compressed. Transcript: {}]\n\n{}", transcript_path.display(), summary)
    })])
}
