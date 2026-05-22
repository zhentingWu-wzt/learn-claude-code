use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const DANGEROUS_PATTERNS: &[&str] = &["rm -rf /", "sudo", "shutdown", "reboot", "> /dev/"];
const MAX_OUTPUT: usize = 50000;

pub fn safe_path(p: &str, workdir: &Path) -> Result<PathBuf, String> {
    let path = workdir.join(p).canonicalize().unwrap_or_else(|_| workdir.join(p));
    let canonical_workdir = workdir.canonicalize().unwrap_or_else(|_| workdir.to_path_buf());
    if !path.starts_with(&canonical_workdir) {
        return Err(format!("Path escapes workspace: {p}"));
    }
    Ok(path)
}

pub fn run_bash(command: &str, workdir: &Path) -> String {
    if DANGEROUS_PATTERNS.iter().any(|d| command.contains(d)) {
        return "Error: Dangerous command blocked".to_string();
    }
    let result = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(workdir)
        .output();
    match result {
        Ok(output) => {
            let mut out = String::from_utf8_lossy(&output.stdout).to_string();
            let err = String::from_utf8_lossy(&output.stderr);
            if !err.is_empty() {
                out.push_str(&err);
            }
            let out = out.trim();
            if out.is_empty() {
                "(no output)".to_string()
            } else if out.len() > MAX_OUTPUT {
                out[..MAX_OUTPUT].to_string()
            } else {
                out.to_string()
            }
        }
        Err(e) => format!("Error: {e}"),
    }
}

pub fn run_bash_with_timeout(command: &str, workdir: &Path, timeout_secs: u64) -> String {
    if DANGEROUS_PATTERNS.iter().any(|d| command.contains(d)) {
        return "Error: Dangerous command blocked".to_string();
    }
    let result = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(workdir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    match result {
        Ok(mut child) => {
            match child.wait_timeout(Duration::from_secs(timeout_secs)) {
                Ok(Some(_status)) => {
                    let output = match child.wait_with_output() {
                        Ok(o) => o,
                        Err(_) => {
                            return "Error: failed to get output".to_string();
                        }
                    };
                    let out = format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ).trim().to_string();
                    if out.is_empty() { "(no output)".to_string() }
                    else if out.len() > MAX_OUTPUT { out[..MAX_OUTPUT].to_string() }
                    else { out }
                }
                Ok(None) => {
                    let _ = child.kill();
                    format!("Error: Timeout ({timeout_secs}s)")
                }
                Err(e) => format!("Error: {e}"),
            }
        }
        Err(e) => format!("Error: {e}"),
    }
}

trait ChildExt {
    fn wait_timeout(&mut self, duration: Duration) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl ChildExt for std::process::Child {
    fn wait_timeout(&mut self, duration: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self.try_wait()? {
            Some(status) => Ok(Some(status)),
            None => {
                std::thread::sleep(duration);
                Ok(self.try_wait()?)
            }
        }
    }
}

pub fn run_read(path: &str, workdir: &Path, limit: Option<usize>) -> String {
    match safe_path(path, workdir) {
        Ok(fp) => match std::fs::read_to_string(&fp) {
            Ok(text) => {
                let all_lines: Vec<&str> = text.lines().collect();
                let mut lines: Vec<String> = all_lines.iter().map(|l| l.to_string()).collect();
                if let Some(lim) = limit {
                    if lim < lines.len() {
                        let more = format!("... ({} more)", all_lines.len() - lim);
                        lines.truncate(lim);
                        lines.push(more);
                    }
                }
                let out: String = lines.join("\n");
                if out.len() > MAX_OUTPUT { out[..MAX_OUTPUT].to_string() } else { out }
            }
            Err(e) => format!("Error: {e}"),
        },
        Err(e) => format!("Error: {e}"),
    }
}

pub fn run_write(path: &str, workdir: &Path, content: &str) -> String {
    match safe_path(path, workdir) {
        Ok(fp) => {
            if let Some(parent) = fp.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return format!("Error: {e}");
                }
            }
            match std::fs::write(&fp, content) {
                Ok(_) => format!("Wrote {} bytes", content.len()),
                Err(e) => format!("Error: {e}"),
            }
        }
        Err(e) => format!("Error: {e}"),
    }
}

pub fn run_edit(path: &str, workdir: &Path, old_text: &str, new_text: &str) -> String {
    match safe_path(path, workdir) {
        Ok(fp) => match std::fs::read_to_string(&fp) {
            Ok(content) => {
                if !content.contains(old_text) {
                    return format!("Error: Text not found in {path}");
                }
                let new_content = content.replacen(old_text, new_text, 1);
                match std::fs::write(&fp, new_content) {
                    Ok(_) => format!("Edited {path}"),
                    Err(e) => format!("Error: {e}"),
                }
            }
            Err(e) => format!("Error: {e}"),
        },
        Err(e) => format!("Error: {e}"),
    }
}
