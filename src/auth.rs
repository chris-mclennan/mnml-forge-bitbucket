//! Bitbucket Cloud app-password loader. Reads
//! `~/.config/mnml-tickets-bitbucket/token` (one line, `chmod 600`).
//!
//! Create one at:
//!   <https://bitbucket.org/account/settings/app-passwords/>
//!
//! Minimum scopes: **Pull requests: Read**. Add **Account: Read** if
//! you want `mode = "mine"` / `mode = "reviewing"` tabs (those need
//! `/2.0/user` to resolve your account_id).

use anyhow::{Context, Result, anyhow};
use std::path::PathBuf;

pub fn token_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("mnml-tickets-bitbucket")
        .join("token")
}

pub fn load_token() -> Result<String> {
    let path = token_path();
    let s =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let token = s.trim().to_string();
    if token.is_empty() {
        return Err(anyhow!(
            "{} is empty — paste your Bitbucket app password",
            path.display()
        ));
    }
    Ok(token)
}
