use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const VALID_MSG_TYPES: &[&str] = &[
    "message",
    "broadcast",
    "shutdown_request",
    "shutdown_response",
    "plan_approval_response",
];

pub struct MessageBus {
    dir: PathBuf,
}

impl MessageBus {
    pub fn new(inbox_dir: &Path) -> Self {
        std::fs::create_dir_all(inbox_dir).ok();
        Self {
            dir: inbox_dir.to_path_buf(),
        }
    }

    pub fn send(
        &self,
        sender: &str,
        to: &str,
        content: &str,
        msg_type: &str,
        extra: Option<Value>,
    ) -> String {
        let valid: HashSet<&str> = VALID_MSG_TYPES.iter().copied().collect();
        if !valid.contains(msg_type) {
            return format!("Error: Invalid type '{msg_type}'. Valid: {VALID_MSG_TYPES:?}");
        }
        let mut msg = serde_json::json!({
            "type": msg_type,
            "from": sender,
            "content": content,
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        });
        if let Some(ex) = extra {
            if let Value::Object(map) = ex {
                if let Some(obj) = msg.as_object_mut() {
                    for (k, v) in map {
                        obj.insert(k, v);
                    }
                }
            }
        }
        let inbox_path = self.dir.join(format!("{to}.jsonl"));
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&inbox_path)
        {
            let _ = writeln!(f, "{}", serde_json::to_string(&msg).unwrap_or_default());
        }
        format!("Sent {msg_type} to {to}")
    }

    pub fn read_inbox(&self, name: &str) -> Vec<Value> {
        let inbox_path = self.dir.join(format!("{name}.jsonl"));
        if !inbox_path.exists() {
            return Vec::new();
        }
        let text = std::fs::read_to_string(&inbox_path).unwrap_or_default();
        let messages: Vec<Value> = text
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        // Drain inbox
        let _ = std::fs::write(&inbox_path, "");
        messages
    }

    pub fn broadcast(&self, sender: &str, content: &str, teammates: &[String]) -> String {
        let mut count = 0;
        for name in teammates {
            if name != sender {
                self.send(sender, name, content, "broadcast", None);
                count += 1;
            }
        }
        format!("Broadcast to {count} teammates")
    }
}
