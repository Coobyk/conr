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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub error: String,
    pub success: String,
    pub dim: String,
    /// When true (default), prefer Omarchy/system colors over these hex values.
    #[serde(default = "default_true")]
    pub follow_system: bool,
}

fn default_true() -> bool {
    true
}

impl Theme {
    /// First-ship spec colors that older clients wrote and never applied.
    pub fn is_legacy_spec(&self) -> bool {
        fn eq(a: &str, b: &str) -> bool {
            a.trim().eq_ignore_ascii_case(b)
        }
        eq(&self.background, "#1D1F21")
            && eq(&self.foreground, "#C5C8C6")
            && eq(&self.accent, "#81A2BE")
            && eq(&self.error, "#CC6666")
            && eq(&self.success, "#B5BD68")
            && eq(&self.dim, "#5A5A5A")
    }

    /// Read the active Omarchy theme (`colors.toml`) so the TUI matches the desktop.
    pub fn detect_system() -> Option<Self> {
        let path = omarchy_colors_path()?;
        let text = std::fs::read_to_string(&path).ok()?;
        Self::from_colors_toml(&text)
    }

    /// Map Omarchy `colors.toml` keys onto the six theme hex slots.
    pub fn from_colors_toml(text: &str) -> Option<Self> {
        let table: toml::Table = toml::from_str(text).ok()?;
        let get = |key: &str| -> Option<String> {
            table.get(key)?.as_str().map(|s| s.trim().to_string())
        };
        let background = get("background")?;
        let foreground = get("foreground")?;
        // Prefer explicit accent, then terminal blue (aether / most Omarchy themes).
        let accent = get("accent")
            .or_else(|| get("blue"))
            .unwrap_or_else(|| "#98b1cd".into());
        let error = get("red").or_else(|| get("bright_red"))?;
        let success = get("green").or_else(|| get("bright_green"))?;
        let dim = get("muted").or_else(|| get("dark_foreground"))?;
        Some(Self {
            background,
            foreground,
            accent,
            error,
            success,
            dim,
            follow_system: true,
        })
    }

    /// Effective theme: system colors when following, else config hex values.
    pub fn effective(&self) -> Theme {
        if self.follow_system {
            if let Some(sys) = Self::detect_system() {
                return sys;
            }
        }
        self.clone()
    }
}

fn omarchy_colors_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = home.join(".local/state/omarchy/current/theme/colors.toml");
    path.exists().then_some(path)
}

impl Default for Theme {
    fn default() -> Self {
        // Prefer the active desktop theme when Omarchy is present.
        if let Some(sys) = Self::detect_system() {
            return sys;
        }
        // Fallback: shipped ink + violet chrome (see ui/theme.rs Palette::INK).
        Self {
            background: "#0a0a0f".into(),
            foreground: "#f0eef6".into(),
            accent: "#a78bfa".into(),
            error: "#f87171".into(),
            success: "#34d399".into(),
            dim: "#8a869a".into(),
            follow_system: true,
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
            let mut cfg = Config::default();
            // Mirror whatever system/config theme resolved so the file is inspectable.
            cfg.theme = cfg.theme.effective();
            let text = toml::to_string_pretty(&cfg)?;
            fs::write(&path, text)?;
            return Ok(cfg);
        }
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text).context("parse config.toml")?;
        // Older clients wrote spec-v0.3 Tomorrow Night colors that the TUI
        // never applied. Rewrite them so honoring theme does not surprise installs.
        if cfg.theme.is_legacy_spec() {
            cfg.theme = Theme::default();
            let _ = cfg.save();
        }
        // Follow the desktop palette on every load (theme switches stay in sync).
        let effective = cfg.theme.effective();
        if cfg.theme != effective {
            cfg.theme = effective;
            let _ = cfg.save();
        }
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = config_dir()?.join("config.toml");
        fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aether_colors_toml() {
        let text = r##"
mode = "dark"
accent = "#98b1cd"
muted = "#616560"
background = "#10190f"
foreground = "#decab2"
red = "#b7aa87"
green = "#acc7a9"
blue = "#98b1cd"
"##;
        let t = Theme::from_colors_toml(text).expect("parse");
        assert_eq!(t.background, "#10190f");
        assert_eq!(t.foreground, "#decab2");
        assert_eq!(t.accent, "#98b1cd");
        assert_eq!(t.error, "#b7aa87");
        assert_eq!(t.success, "#acc7a9");
        assert_eq!(t.dim, "#616560");
        assert!(t.follow_system);
    }

    #[test]
    fn effective_without_system_uses_config() {
        let mut t = Theme {
            background: "#111111".into(),
            foreground: "#eeeeee".into(),
            accent: "#ff00ff".into(),
            error: "#ff0000".into(),
            success: "#00ff00".into(),
            dim: "#888888".into(),
            follow_system: false,
        };
        assert_eq!(t.effective(), t);
        t.follow_system = true;
        // On machines without Omarchy, system detection fails and config wins.
        if Theme::detect_system().is_none() {
            assert_eq!(t.effective(), t);
        }
    }
}
