use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiResponse {
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
}

pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl AnthropicClient {
    pub fn new(api_key: &str, base_url: &str, model: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
        }
    }

    pub async fn create_message(
        &self,
        system: &str,
        messages: &[Value],
        tools: &[ToolDef],
        max_tokens: u32,
    ) -> anyhow::Result<ApiResponse> {
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": messages,
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::to_value(tools)?;
        }

        let url = format!("{}/v1/messages", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("API error {status}: {text}");
        }

        let api_response: ApiResponse = resp.json().await?;
        Ok(api_response)
    }

    /// Simplified call for internal summarization (no tools).
    pub async fn create_simple_message(
        &self,
        messages: &[Value],
        max_tokens: u32,
    ) -> anyhow::Result<ApiResponse> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "messages": messages,
        });

        let url = format!("{}/v1/messages", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("API error {status}: {text}");
        }

        let api_response: ApiResponse = resp.json().await?;
        Ok(api_response)
    }
}

// Helper: build a user message with text content
pub fn user_text(text: &str) -> Value {
    serde_json::json!({"role": "user", "content": text})
}

// Helper: build an assistant message from API content blocks
pub fn assistant_message(content: &[ContentBlock]) -> Value {
    serde_json::json!({"role": "assistant", "content": content})
}

// Helper: build a user message with tool results
pub fn tool_result_message(results: Vec<ContentBlock>) -> Value {
    serde_json::json!({"role": "user", "content": results})
}

// Extract text from content blocks
pub fn extract_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

// Build tool definitions (reusable across scenarios)
pub fn bash_tool() -> ToolDef {
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

pub fn read_file_tool() -> ToolDef {
    ToolDef {
        name: "read_file".to_string(),
        description: "Read file contents.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "limit": {"type": "integer"}},
            "required": ["path"]
        }),
    }
}

pub fn write_file_tool() -> ToolDef {
    ToolDef {
        name: "write_file".to_string(),
        description: "Write content to file.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
            "required": ["path", "content"]
        }),
    }
}

pub fn edit_file_tool() -> ToolDef {
    ToolDef {
        name: "edit_file".to_string(),
        description: "Replace exact text in file.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_text": {"type": "string"},
                "new_text": {"type": "string"}
            },
            "required": ["path", "old_text", "new_text"]
        }),
    }
}

pub fn base_tools() -> Vec<ToolDef> {
    vec![bash_tool(), read_file_tool(), write_file_tool(), edit_file_tool()]
}
