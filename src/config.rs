use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct HelideConfig {
    pub font: FontConfig,
    pub terminal: TerminalConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub split_ratio: f32,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self { split_ratio: 0.7 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            family: "monospace".to_string(),
            size: 14.0,
        }
    }
}

impl HelideConfig {
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => config,
                Err(err) => {
                    eprintln!("helide: bad config {}: {err}", path.display());
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }
}

pub fn config_path() -> PathBuf {
    // Check XDG-style first (~/.config/helide), then platform config dir
    let candidates = [
        dirs::home_dir().map(|d| d.join(".config/helide/config.toml")),
        dirs::config_dir().map(|d| d.join("helide/config.toml")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return candidate;
        }
    }
    // Default to XDG-style path
    dirs::home_dir()
        .map(|d| d.join(".config/helide/config.toml"))
        .unwrap_or_else(|| PathBuf::from("helide.toml"))
}
