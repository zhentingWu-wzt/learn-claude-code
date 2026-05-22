use std::path::PathBuf;

pub struct Config {
    pub api_key: String,
    pub model_id: String,
    pub base_url: String,
    pub workdir: PathBuf,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();

        if std::env::var("ANTHROPIC_BASE_URL").is_ok() {
            std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        }

        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY not set"))?;
        let model_id = std::env::var("MODEL_ID")
            .map_err(|_| anyhow::anyhow!("MODEL_ID not set"))?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
        let workdir = std::env::current_dir()?;

        Ok(Config { api_key, model_id, base_url, workdir })
    }
}
