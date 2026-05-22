use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub status: String,
}

pub struct TodoManager {
    pub items: Vec<TodoItem>,
}

impl TodoManager {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn update(&mut self, items: Vec<Value>) -> Result<String, String> {
        if items.len() > 20 {
            return Err("Max 20 todos allowed".to_string());
        }
        let mut validated = Vec::new();
        let mut in_progress_count = 0;
        for (i, item) in items.iter().enumerate() {
            let text = item.get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let status = item.get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending")
                .to_lowercase();
            let id = item.get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&format!("{}", i + 1))
                .to_string();
            if text.is_empty() {
                return Err(format!("Item {id}: text required"));
            }
            if !["pending", "in_progress", "completed"].contains(&status.as_str()) {
                return Err(format!("Item {id}: invalid status '{status}'"));
            }
            if status == "in_progress" {
                in_progress_count += 1;
            }
            validated.push(TodoItem { id, text, status });
        }
        if in_progress_count > 1 {
            return Err("Only one task can be in_progress at a time".to_string());
        }
        self.items = validated;
        Ok(self.render())
    }

    pub fn render(&self) -> String {
        if self.items.is_empty() {
            return "No todos.".to_string();
        }
        let mut lines = Vec::new();
        for item in &self.items {
            let marker = match item.status.as_str() {
                "pending" => "[ ]",
                "in_progress" => "[>]",
                "completed" => "[x]",
                _ => "[?]",
            };
            lines.push(format!("{marker} #{}: {}", item.id, item.text));
        }
        let done = self.items.iter().filter(|t| t.status == "completed").count();
        lines.push(format!("\n({done}/{}) completed", self.items.len()));
        lines.join("\n")
    }

    pub fn has_open_items(&self) -> bool {
        self.items.iter().any(|t| t.status != "completed")
    }
}

use serde_json::Value;
