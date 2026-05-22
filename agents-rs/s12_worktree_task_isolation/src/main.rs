//! Harness: directory isolation -- parallel execution lanes that never collide.
//!
//! Directory-level isolation for parallel task execution.
//! Tasks are the control plane and worktrees are the execution plane.

use anyhow::Result;
use harness::{AnthropicClient, Config, ContentBlock, ToolDef, TaskManager,
    assistant_message, tool_result_message, user_text, base_tools};
use std::io::{self, Write};
use std::sync::Mutex;

struct WorktreeEntry {
    name: String,
    path: String,
    branch: String,
    task_id: Option<u32>,
    status: String,
}

struct WorktreeIndex {
    worktrees: Vec<WorktreeEntry>,
}

struct WorktreeManager {
    repo_root: std::path::PathBuf,
    dir: std::path::PathBuf,
    index_path: std::path::PathBuf,
    git_available: bool,
}

impl WorktreeManager {
    fn new(repo_root: &std::path::Path, _tasks: &std::path::Path) -> Self {
        let dir = repo_root.join(".worktrees");
        std::fs::create_dir_all(&dir).ok();
        let index_path = dir.join("index.json");
        if !index_path.exists() {
            std::fs::write(&index_path, r#"{"worktrees": []}"#).ok();
        }
        let git_available = std::process::Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(repo_root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        Self { repo_root: repo_root.to_path_buf(), dir, index_path, git_available }
    }

    fn load_index(&self) -> WorktreeIndex {
        let text = std::fs::read_to_string(&self.index_path).unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let worktrees = v["worktrees"].as_array()
            .map(|arr| arr.iter().filter_map(|w| {
                Some(WorktreeEntry {
                    name: w["name"].as_str()?.to_string(),
                    path: w["path"].as_str()?.to_string(),
                    branch: w["branch"].as_str().unwrap_or("-").to_string(),
                    task_id: w["task_id"].as_u64().map(|id| id as u32),
                    status: w["status"].as_str().unwrap_or("unknown").to_string(),
                })
            }).collect())
            .unwrap_or_default();
        WorktreeIndex { worktrees }
    }

    fn save_index(&self, index: &WorktreeIndex) {
        let worktrees: Vec<serde_json::Value> = index.worktrees.iter().map(|w| {
            let mut entry = serde_json::json!({
                "name": &w.name,
                "path": &w.path,
                "branch": &w.branch,
                "status": &w.status,
            });
            if let Some(tid) = w.task_id { entry["task_id"] = serde_json::json!(tid); }
            entry
        }).collect();
        std::fs::write(&self.index_path, serde_json::to_string_pretty(&serde_json::json!({"worktrees": worktrees})).unwrap_or_default()).ok();
    }

    fn create(&mut self, name: &str, task_id: Option<u32>, base_ref: &str) -> Result<String, String> {
        if !self.git_available { return Err("Not in a git repository. worktree tools require git.".to_string()); }
        let re = regex::Regex::new(r"^[A-Za-z0-9._-]{1,40}$").unwrap();
        if !re.is_match(name) { return Err("Invalid worktree name. Use 1-40 chars: letters, numbers, ., _, -".to_string()); }
        let index = self.load_index();
        if index.worktrees.iter().any(|w| w.name == name) { return Err(format!("Worktree '{name}' already exists in index")); }

        let path = self.dir.join(name);
        let branch = format!("wt/{name}");
        let output = std::process::Command::new("git")
            .args(["worktree", "add", "-b", &branch, &path.to_string_lossy(), base_ref])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(err.trim().to_string());
        }

        let mut index = self.load_index();
        let entry = WorktreeEntry {
            name: name.to_string(),
            path: path.to_string_lossy().to_string(),
            branch: branch.clone(),
            task_id,
            status: "active".to_string(),
        };
        let result = serde_json::to_string_pretty(&serde_json::json!({
            "name": name, "path": path.to_string_lossy(), "branch": branch,
            "task_id": task_id, "status": "active"
        })).unwrap_or_default();
        index.worktrees.push(entry);
        self.save_index(&index);
        Ok(result)
    }

    fn list_all(&self) -> String {
        let index = self.load_index();
        if index.worktrees.is_empty() { return "No worktrees in index.".to_string(); }
        index.worktrees.iter()
            .map(|w| {
                let task_suffix = w.task_id.map(|tid| format!(" task={tid}")).unwrap_or_default();
                format!("[{}] {} -> {} ({}){task_suffix}", w.status, w.name, w.path, w.branch)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn status(&self, name: &str) -> String {
        let index = self.load_index();
        let wt = match index.worktrees.iter().find(|w| w.name == name) {
            Some(w) => w, None => return format!("Error: Unknown worktree '{name}'"),
        };
        let path = std::path::Path::new(&wt.path);
        if !path.exists() { return format!("Error: Worktree path missing: {}", wt.path); }
        let output = std::process::Command::new("git")
            .args(["status", "--short", "--branch"])
            .current_dir(path)
            .output();
        match output {
            Ok(o) => {
                let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
                let text = text.trim();
                if text.is_empty() { "Clean worktree".to_string() } else { text.to_string() }
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    fn run(&self, name: &str, command: &str) -> String {
        let index = self.load_index();
        let wt = match index.worktrees.iter().find(|w| w.name == name) {
            Some(w) => w, None => return format!("Error: Unknown worktree '{name}'"),
        };
        let path = std::path::Path::new(&wt.path);
        if !path.exists() { return format!("Error: Worktree path missing: {}", wt.path); }
        harness::tools::run_bash_with_timeout(command, path, 300)
    }

    fn remove(&mut self, name: &str, force: bool, _complete_task: bool) -> String {
        let index = self.load_index();
        let wt = match index.worktrees.iter().find(|w| w.name == name) {
            Some(w) => w, None => return format!("Error: Unknown worktree '{name}'"),
        };
        let path = wt.path.clone();

        let mut args: Vec<String> = vec!["worktree".into(), "remove".into()];
        if force { args.push("--force".to_string()); }
        args.push(path.clone());
        let output = std::process::Command::new("git")
            .args(&args)
            .current_dir(&self.repo_root)
            .output();
        if let Ok(o) = &output {
            if !o.status.success() {
                let err = String::from_utf8_lossy(&o.stderr);
                return format!("Error removing worktree: {}", err.trim());
            }
        }

        let mut index = self.load_index();
        for item in index.worktrees.iter_mut() {
            if item.name == name {
                item.status = "removed".to_string();
            }
        }
        self.save_index(&index);
        format!("Removed worktree '{name}'")
    }
}

fn worktree_tools() -> Vec<ToolDef> {
    let mut tools = base_tools();
    tools.push(ToolDef { name: "task_create".into(), description: "Create a new task on the shared task board.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"subject": {"type": "string"}, "description": {"type": "string"}}, "required": ["subject"]}) });
    tools.push(ToolDef { name: "task_list".into(), description: "List all tasks.".into(), input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    tools.push(ToolDef { name: "task_get".into(), description: "Get task details by ID.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"task_id": {"type": "integer"}}, "required": ["task_id"]}) });
    tools.push(ToolDef { name: "task_update".into(), description: "Update task status or owner.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"task_id": {"type": "integer"}, "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}, "owner": {"type": "string"}}, "required": ["task_id"]}) });
    tools.push(ToolDef { name: "task_bind_worktree".into(), description: "Bind a task to a worktree name.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"task_id": {"type": "integer"}, "worktree": {"type": "string"}, "owner": {"type": "string"}}, "required": ["task_id", "worktree"]}) });
    tools.push(ToolDef { name: "worktree_create".into(), description: "Create a git worktree and optionally bind it to a task.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}, "task_id": {"type": "integer"}, "base_ref": {"type": "string"}}, "required": ["name"]}) });
    tools.push(ToolDef { name: "worktree_list".into(), description: "List worktrees tracked in .worktrees/index.json.".into(), input_schema: serde_json::json!({"type": "object", "properties": {}}) });
    tools.push(ToolDef { name: "worktree_status".into(), description: "Show git status for one worktree.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}) });
    tools.push(ToolDef { name: "worktree_run".into(), description: "Run a shell command in a named worktree directory.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}, "command": {"type": "string"}}, "required": ["name", "command"]}) });
    tools.push(ToolDef { name: "worktree_remove".into(), description: "Remove a worktree.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}, "force": {"type": "boolean"}, "complete_task": {"type": "boolean"}}, "required": ["name"]}) });
    tools.push(ToolDef { name: "worktree_keep".into(), description: "Mark a worktree as kept.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}) });
    tools.push(ToolDef { name: "worktree_events".into(), description: "List recent worktree/task lifecycle events.".into(), input_schema: serde_json::json!({"type": "object", "properties": {"limit": {"type": "integer"}}}) });
    tools
}

async fn agent_loop(
    client: &AnthropicClient,
    messages: &mut Vec<serde_json::Value>,
    workdir: &std::path::Path,
    tasks: &Mutex<TaskManager>,
    worktrees: &Mutex<WorktreeManager>,
) {
    let system = format!(
        "You are a coding agent at {}. Use task + worktree tools for multi-task work. \
         For parallel or risky changes: create tasks, allocate worktree lanes, \
         run commands in those lanes, then choose keep/remove for closeout.",
        workdir.display()
    );
    let tools = worktree_tools();

    loop {
        let resp = match client.create_message(&system, messages, &tools, 8000).await {
            Ok(r) => r, Err(e) => { eprintln!("API error: {e}"); return; }
        };
        let stop = resp.stop_reason.as_deref() != Some("tool_use");
        messages.push(assistant_message(&resp.content));
        if stop { return; }

        let mut results = Vec::new();
        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let output = match &**name {
                    "bash" => harness::tools::run_bash(input["command"].as_str().unwrap_or(""), workdir),
                    "read_file" => harness::tools::run_read(input["path"].as_str().unwrap_or(""), workdir, input["limit"].as_u64().map(|l| l as usize)),
                    "write_file" => harness::tools::run_write(input["path"].as_str().unwrap_or(""), workdir, input["content"].as_str().unwrap_or("")),
                    "edit_file" => harness::tools::run_edit(input["path"].as_str().unwrap_or(""), workdir, input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or("")),
                    "task_create" => { let mut t = tasks.lock().unwrap(); t.create(input["subject"].as_str().unwrap_or(""), input["description"].as_str().unwrap_or("")).unwrap_or_else(|e| format!("Error: {e}")) }
                    "task_list" => tasks.lock().unwrap().list_all(),
                    "task_get" => { let t = tasks.lock().unwrap(); t.get(input["task_id"].as_u64().unwrap_or(0) as u32).unwrap_or_else(|e| format!("Error: {e}")) }
                    "task_update" => { let mut t = tasks.lock().unwrap(); t.update(input["task_id"].as_u64().unwrap_or(0) as u32, input["status"].as_str(), None, None).unwrap_or_else(|e| format!("Error: {e}")) }
                    "task_bind_worktree" => { let mut t = tasks.lock().unwrap(); t.update(input["task_id"].as_u64().unwrap_or(0) as u32, None, None, None).unwrap_or_else(|e| format!("Error: {e}")) }
                    "worktree_create" => { let mut wt = worktrees.lock().unwrap(); wt.create(input["name"].as_str().unwrap_or(""), input["task_id"].as_u64().map(|id| id as u32), input["base_ref"].as_str().unwrap_or("HEAD")).unwrap_or_else(|e| format!("Error: {e}")) }
                    "worktree_list" => worktrees.lock().unwrap().list_all(),
                    "worktree_status" => worktrees.lock().unwrap().status(input["name"].as_str().unwrap_or("")),
                    "worktree_run" => worktrees.lock().unwrap().run(input["name"].as_str().unwrap_or(""), input["command"].as_str().unwrap_or("")),
                    "worktree_remove" => { let mut wt = worktrees.lock().unwrap(); wt.remove(input["name"].as_str().unwrap_or(""), input["force"].as_bool().unwrap_or(false), input["complete_task"].as_bool().unwrap_or(false)) }
                    "worktree_keep" => format!("Kept worktree '{}'", input["name"].as_str().unwrap_or("")),
                    "worktree_events" => "No events yet.".to_string(),
                    _ => format!("Unknown tool: {name}"),
                };
                println!("> {name}:");
                println!("{}", &output[..output.len().min(200)]);
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

    // Detect repo root
    let repo_root = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&workdir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or_else(|| workdir.clone());

    println!("Repo root for s12: {}", repo_root.display());

    let tasks = Mutex::new(TaskManager::new(&repo_root.join(".tasks")));
    let worktrees = Mutex::new(WorktreeManager::new(&repo_root, &repo_root.join(".tasks")));
    let mut history: Vec<serde_json::Value> = Vec::new();

    loop {
        print!("\x1b[36ms12 >> \x1b[0m");
        io::stdout().flush()?;
        let mut query = String::new();
        match io::stdin().read_line(&mut query) { Ok(0) | Err(_) => break, _ => {} }
        let query = query.trim();
        if query.is_empty() || query == "q" || query == "exit" { break; }
        history.push(user_text(query));
        agent_loop(&client, &mut history, &workdir, &tasks, &worktrees).await;

        if let Some(last) = history.last() {
            if let Some(content) = last["content"].as_array() {
                for block in content { if let Some(text) = block["text"].as_str() { println!("{text}"); } }
            } else if let Some(text) = last["content"].as_str() { println!("{text}"); }
        }
        println!();
    }
    Ok(())
}
