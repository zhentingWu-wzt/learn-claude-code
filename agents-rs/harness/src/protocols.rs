use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ShutdownRequest {
    pub target: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct PlanRequest {
    pub from: String,
    pub plan: String,
    pub status: String,
}

pub struct ProtocolTracker {
    pub shutdown_requests: Mutex<HashMap<String, ShutdownRequest>>,
    pub plan_requests: Mutex<HashMap<String, PlanRequest>>,
    pub plan_approvals: Mutex<HashMap<String, String>>,
    pub plan_requirements: Mutex<HashMap<String, bool>>,
}

impl ProtocolTracker {
    pub fn new() -> Self {
        Self {
            shutdown_requests: Mutex::new(HashMap::new()),
            plan_requests: Mutex::new(HashMap::new()),
            plan_approvals: Mutex::new(HashMap::new()),
            plan_requirements: Mutex::new(HashMap::new()),
        }
    }
}

const RISKY_KEYWORDS: &[&str] = &["risky", "refactor", "refactoring", "major", "rewrite"];
const PLAN_GATED_TOOLS: &[&str] = &["write_file", "edit_file"];
const STRICT_PLAN_GATED_TOOLS: &[&str] = &["bash", "write_file", "edit_file"];

pub fn requires_plan(prompt: &str) -> bool {
    let lowered = prompt.to_lowercase();
    RISKY_KEYWORDS.iter().any(|k| lowered.contains(k))
}

pub fn can_execute_tool(
    tracker: &ProtocolTracker,
    name: &str,
    tool_name: &str,
) -> Result<(), String> {
    let required = tracker.plan_requirements.lock().unwrap().get(name).copied().unwrap_or(false);
    let state = tracker.plan_approvals.lock().unwrap().get(name).cloned().unwrap_or_else(|| "not_required".to_string());

    if tool_name == "plan_approval" {
        if state == "pending" {
            return Err("Plan approval already pending. Wait for lead response.".to_string());
        }
        return Ok(());
    }

    if !required {
        return Ok(());
    }

    let gated = if state == "not_required" {
        STRICT_PLAN_GATED_TOOLS
    } else {
        PLAN_GATED_TOOLS
    };

    if gated.contains(&tool_name) && state != "approved" {
        if state == "pending" {
            return Err("Plan approval pending. Wait before taking action.".to_string());
        }
        return Err("Plan approval required before using this tool.".to_string());
    }

    Ok(())
}
