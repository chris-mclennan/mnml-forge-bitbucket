//! Config file at `~/.config/mnml-forge-bitbucket.toml`. First run
//! writes the scaffold + exits with instructions.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Atlassian / Bitbucket Cloud account email (used as the
    /// username half of HTTP Basic auth with the app password).
    pub email: String,
    /// Default workspace slug — the part before the `/` in
    /// `bitbucket.org/<workspace>/<repo>`. Tabs can override this
    /// per-row via `workspace = "..."`.
    pub workspace: String,
    /// Polling interval. `0` disables auto-refresh; user can still
    /// press `r` to refresh the active tab. Default 60s.
    #[serde(default = "default_refresh")]
    pub refresh_interval_secs: u64,
    /// Tab list — at least one required.
    #[serde(default)]
    pub tabs: Vec<Tab>,
}

fn default_refresh() -> u64 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    /// Human label shown in the tab strip.
    pub name: String,
    /// Override the default workspace for this tab.
    #[serde(default)]
    pub workspace: Option<String>,
    /// Repository slug (the part after `<workspace>/`). Required
    /// for the default `repo` mode; ignored by `mode = "mine"` and
    /// `mode = "reviewing"` (which span the whole workspace).
    #[serde(default)]
    pub repo: Option<String>,
    /// PR state filter — `OPEN` (default), `MERGED`, `DECLINED`,
    /// `SUPERSEDED`.
    #[serde(default = "default_state")]
    pub state: String,
    /// Optional mode for cross-repo tabs:
    ///   - omitted ⇒ literal per-repo lookup (needs `repo`)
    ///   - `mine` ⇒ PRs you opened, across the workspace
    ///   - `reviewing` ⇒ PRs where you are a reviewer
    ///
    /// Both auto-modes use Bitbucket's `q=` BBQL via the workspace
    /// PR endpoint and resolve the current user's `account_id` at
    /// load time via `/2.0/user`.
    #[serde(default)]
    pub mode: Option<String>,
    /// Optional raw BBQL appended to the auto-mode query (or used
    /// as the only filter when `mode` and `repo` are both unset).
    /// Example: `state = "OPEN" AND author.account_id = "{abc}"`.
    #[serde(default)]
    pub q: Option<String>,
}

fn default_state() -> String {
    "OPEN".to_string()
}

impl Config {
    pub const EXAMPLE: &'static str = r##"# mnml-forge-bitbucket config. Edit and re-run.
#
# Required:
#   email        — your Atlassian / Bitbucket account email
#   workspace    — default workspace slug (bitbucket.org/<workspace>/<repo>)

email     = "you@example.com"
workspace = "your-workspace-slug"

# Auto-refresh in seconds. 0 disables; user can still press `r`.
refresh_interval_secs = 60

# ── Tabs ─────────────────────────────────────────────────────────
# Each `[[tabs]]` entry is one tab. Switched via 1-9 (or click) and
# rendered left→right. Three shapes:
#
#   1. Repo-scoped — `repo = "..."` (workspace falls back to default)
#   2. `mode = "mine"` — PRs you opened across the default workspace
#   3. `mode = "reviewing"` — PRs you're a reviewer on
#
# `state` defaults to OPEN. Other values: MERGED / DECLINED / SUPERSEDED.

[[tabs]]
name = "Mine"
mode = "mine"

[[tabs]]
name = "Reviewing"
mode = "reviewing"

[[tabs]]
name = "your-repo PRs"
repo = "your-repo"
state = "OPEN"

# [[tabs]]
# name = "Recently merged"
# repo = "your-repo"
# state = "MERGED"
"##;

    pub fn validate(&self) -> Result<()> {
        if self.email.trim().is_empty() {
            return Err(anyhow!("config: `email` is required"));
        }
        if self.workspace.trim().is_empty() {
            return Err(anyhow!("config: `workspace` is required"));
        }
        if self.tabs.is_empty() {
            return Err(anyhow!("config: at least one [[tabs]] entry required"));
        }
        for (i, t) in self.tabs.iter().enumerate() {
            let valid_state = matches!(
                t.state.as_str(),
                "OPEN" | "MERGED" | "DECLINED" | "SUPERSEDED"
            );
            if !valid_state {
                return Err(anyhow!(
                    "tab #{i} ({}): state must be OPEN / MERGED / DECLINED / SUPERSEDED, got `{}`",
                    t.name,
                    t.state
                ));
            }
            if let Some(mode) = &t.mode {
                if mode != "mine" && mode != "reviewing" {
                    return Err(anyhow!(
                        "tab #{i} ({}): mode must be `mine` or `reviewing`, got `{mode}`",
                        t.name
                    ));
                }
            } else if t.repo.is_none() && t.q.is_none() {
                return Err(anyhow!(
                    "tab #{i} ({}): one of `mode`, `repo`, or `q` is required",
                    t.name
                ));
            }
        }
        Ok(())
    }
}

pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("mnml-forge-bitbucket.toml")
}

pub fn load() -> Result<Config> {
    let path = config_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, Config::EXAMPLE)?;
        return Err(anyhow!(
            "wrote config template to {} — edit it (set email + workspace), then re-run",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(&path)?;
    let cfg: Config = toml::from_str(&text)?;
    cfg.validate()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses_and_validates() {
        // The example uses placeholder email/workspace; substitute
        // valid ones before asserting validate() passes.
        let mut cfg: Config = toml::from_str(Config::EXAMPLE).expect("example parses");
        cfg.email = "alice@example.com".into();
        cfg.workspace = "tattlecorp".into();
        cfg.validate().expect("example validates after fill-in");
        assert!(cfg.tabs.len() >= 3);
    }

    #[test]
    fn validate_rejects_missing_email() {
        let raw = r##"
email = ""
workspace = "ws"
[[tabs]]
name = "Mine"
mode = "mine"
"##;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_missing_workspace() {
        let raw = r##"
email = "a@b.com"
workspace = ""
[[tabs]]
name = "Mine"
mode = "mine"
"##;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_bad_state() {
        let raw = r##"
email = "a@b.com"
workspace = "ws"
[[tabs]]
name = "Bad"
mode = "mine"
state = "PENDING"
"##;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_bad_mode() {
        let raw = r##"
email = "a@b.com"
workspace = "ws"
[[tabs]]
name = "Bad"
mode = "haha"
"##;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_tab_with_no_mode_repo_or_q() {
        let raw = r##"
email = "a@b.com"
workspace = "ws"
[[tabs]]
name = "Bad"
"##;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_no_tabs() {
        let raw = r##"
email = "a@b.com"
workspace = "ws"
"##;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert!(cfg.validate().is_err());
    }
}
