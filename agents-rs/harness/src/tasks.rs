use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: u32,
    pub subject: String,
    #[serde(default)]
    pub description: String,
    pub status: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub blocked_by: Vec<u32>,
}

pub struct TaskManager {
    dir: PathBuf,
    next_id: u32,
}

impl TaskManager {
    pub fn new(tasks_dir: &Path) -> Self {
        std::fs::create_dir_all(tasks_dir).ok();
        let max_id = std::fs::read_dir(tasks_dir)
            .ok()
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name();
                        let name = name.to_string_lossy();
                        name.strip_prefix("task_")
                            .and_then(|s| s.strip_suffix(".json"))
                            .and_then(|s| s.parse::<u32>().ok())
                    })
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        Self {
            dir: tasks_dir.to_path_buf(),
            next_id: max_id + 1,
        }
    }

    fn path(&self, id: u32) -> PathBuf {
        self.dir.join(format!("task_{id}.json"))
    }

    fn load(&self, id: u32) -> Result<Task, String> {
        let path = self.path(id);
        if !path.exists() {
            return Err(format!("Task {id} not found"));
        }
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    fn save(&self, task: &Task) -> Result<(), String> {
        let path = self.path(task.id);
        let json = serde_json::to_string_pretty(task).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn create(&mut self, subject: &str, description: &str) -> Result<String, String> {
        let task = Task {
            id: self.next_id,
            subject: subject.to_string(),
            description: description.to_string(),
            status: "pending".to_string(),
            owner: String::new(),
            blocked_by: Vec::new(),
        };
        self.save(&task)?;
        self.next_id += 1;
        Ok(serde_json::to_string_pretty(&task).map_err(|e| e.to_string())?)
    }

    pub fn get(&self, id: u32) -> Result<String, String> {
        let task = self.load(id)?;
        Ok(serde_json::to_string_pretty(&task).map_err(|e| e.to_string())?)
    }

    pub fn exists(&self, id: u32) -> bool {
        self.path(id).exists()
    }

    pub fn update(
        &mut self,
        id: u32,
        status: Option<&str>,
        add_blocked_by: Option<Vec<u32>>,
        remove_blocked_by: Option<Vec<u32>>,
    ) -> Result<String, String> {
        let mut task = self.load(id)?;
        if let Some(s) = status {
            if !["pending", "in_progress", "completed"].contains(&s) {
                return Err(format!("Invalid status: {s}"));
            }
            task.status = s.to_string();
            if s == "completed" {
                self.clear_dependency(id)?;
            }
        }
        if let Some(add) = add_blocked_by {
            let mut set = std::collections::HashSet::new();
            set.extend(&task.blocked_by);
            set.extend(&add);
            task.blocked_by = set.into_iter().collect();
        }
        if let Some(remove) = remove_blocked_by {
            let remove_set: std::collections::HashSet<_> = remove.into_iter().collect();
            task.blocked_by.retain(|x| !remove_set.contains(x));
        }
        self.save(&task)?;
        Ok(serde_json::to_string_pretty(&task).map_err(|e| e.to_string())?)
    }

    fn clear_dependency(&self, completed_id: u32) -> Result<(), String> {
        let entries = std::fs::read_dir(&self.dir).map_err(|e| e.to_string())?;
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.to_string_lossy().ends_with(".json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            if let Ok(mut task) = serde_json::from_str::<Task>(&text) {
                if task.blocked_by.contains(&completed_id) {
                    task.blocked_by.retain(|x| *x != completed_id);
                    self.save(&task)?;
                }
            }
        }
        Ok(())
    }

    pub fn list_all(&self) -> String {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(_) => return "No tasks.".to_string(),
        };
        let mut tasks: Vec<Task> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                serde_json::from_str(&text).ok()
            })
            .collect();
        tasks.sort_by_key(|t| t.id);
        if tasks.is_empty() {
            return "No tasks.".to_string();
        }
        let mut lines = Vec::new();
        for t in &tasks {
            let marker = match t.status.as_str() {
                "pending" => "[ ]",
                "in_progress" => "[>]",
                "completed" => "[x]",
                _ => "[?]",
            };
            let blocked = if t.blocked_by.is_empty() {
                String::new()
            } else {
                format!(" (blocked by: {:?})", t.blocked_by)
            };
            lines.push(format!("{marker} #{}: {}{blocked}", t.id, t.subject));
        }
        lines.join("\n")
    }

    pub fn claim(&mut self, id: u32, owner: &str) -> Result<String, String> {
        let mut task = self.load(id)?;
        task.owner = owner.to_string();
        task.status = "in_progress".to_string();
        self.save(&task)?;
        Ok(format!("Claimed task #{id} for {owner}"))
    }

    pub fn scan_unclaimed(&self) -> Vec<Task> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let mut tasks: Vec<Task> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                serde_json::from_str(&text).ok()
            })
            .filter(|t: &Task| {
                t.status == "pending" && t.owner.is_empty() && t.blocked_by.is_empty()
            })
            .collect();
        tasks.sort_by_key(|t| t.id);
        tasks
    }
}
