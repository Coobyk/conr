use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server_url: String,
    #[serde(default)]
    pub theme: Theme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub error: String,
    pub success: String,
    pub dim: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: "#1D1F21".into(),
            foreground: "#C5C8C6".into(),
            accent: "#81A2BE".into(),
            error: "#CC6666".into(),
            success: "#B5BD68".into(),
            dim: "#5A5A5A".into(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:8080".into(),
            theme: Theme::default(),
        }
    }
}

pub fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("no XDG config dir")?
        .join("conr");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn data_dir() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("no XDG data dir")?
        .join("conr");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_dir()?.join("config.toml");
        if !path.exists() {
            let cfg = Config::default();
            let text = toml::to_string_pretty(&cfg)?;
            fs::write(&path, text)?;
            return Ok(cfg);
        }
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let cfg: Config = toml::from_str(&text).context("parse config.toml")?;
        Ok(cfg)
    }

    #[allow(dead_code)]
    pub fn save(&self) -> Result<()> {
        let path = config_dir()?.join("config.toml");
        fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}
