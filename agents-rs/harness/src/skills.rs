use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

pub struct Skill {
    pub meta: HashMap<String, String>,
    pub body: String,
}

pub struct SkillLoader {
    pub skills: HashMap<String, Skill>,
}

impl SkillLoader {
    pub fn new(skills_dir: &Path) -> Self {
        let mut skills = HashMap::new();
        if !skills_dir.exists() {
            return Self { skills };
        }
        let re = Regex::new(r"^---\n(.*?)\n---\n(.*)").unwrap();
        for entry in walkdir::WalkDir::new(skills_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
                let text = match std::fs::read_to_string(path) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let (meta, body) = if let Some(caps) = re.captures(&text) {
                    let mut m = HashMap::new();
                    for line in caps[1].lines() {
                        if let Some((k, v)) = line.split_once(':') {
                            m.insert(k.trim().to_string(), v.trim().to_string());
                        }
                    }
                    (m, caps[2].trim().to_string())
                } else {
                    (HashMap::new(), text)
                };
                let name = meta.get("name")
                    .cloned()
                    .unwrap_or_else(|| path.parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default());
                skills.insert(name, Skill { meta, body });
            }
        }
        Self { skills }
    }

    pub fn get_descriptions(&self) -> String {
        if self.skills.is_empty() {
            return "(no skills available)".to_string();
        }
        let mut lines = Vec::new();
        for (name, skill) in &self.skills {
            let desc = skill.meta.get("description").map(|s| s.as_str()).unwrap_or("No description");
            lines.push(format!("  - {name}: {desc}"));
        }
        lines.join("\n")
    }

    pub fn get_content(&self, name: &str) -> String {
        match self.skills.get(name) {
            Some(skill) => format!("<skill name=\"{name}\">\n{}\n</skill>", skill.body),
            None => {
                let available: Vec<&String> = self.skills.keys().collect();
                let avail_str: Vec<&str> = available.iter().map(|s| s.as_str()).collect();
                format!("Error: Unknown skill '{name}'. Available: {}", avail_str.join(", "))
            }
        }
    }
}
