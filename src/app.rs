//! App state — what's loaded, what's selected, the configured query
//! for each tab.

use crate::bitbucket::{Client, Comment, PullRequest};
use crate::config::{Config, Tab};
use anyhow::Result;
use std::collections::HashMap;

pub struct App {
    pub cfg: Config,
    pub client: Client,
    /// Authenticated user's account_id, resolved at startup. Drives
    /// the approve/unapprove toggle + the "✓ approved by you" badge.
    /// `None` ⇒ no Account:Read scope or whoami failed.
    pub me_account_id: Option<String>,
    pub tabs: Vec<TabState>,
    pub active_tab: usize,
    pub status: String,
    /// Right-half detail panel visibility (toggled with `d`).
    pub details_visible: bool,
    /// First-line offset into the detail body (`Ctrl+U/D` scroll).
    pub details_scroll: u16,
    /// Per-PR detail cache, keyed by (workspace, repo, id). Survives
    /// arrow-key navigation so re-selecting a focused row doesn't
    /// re-fetch.
    pub detail_cache: HashMap<(String, String, i64), DetailEntry>,
    /// In-flight detail key (so we don't fire a second fetch on top of
    /// a pending one). `None` when idle.
    pub detail_in_flight: Option<(String, String, i64)>,
}

/// Cached PR detail + comments. Fetched lazily on first focus while
/// the detail panel is open.
#[derive(Debug, Clone)]
pub struct DetailEntry {
    pub pr: PullRequest,
    pub comments: Vec<Comment>,
}

pub struct TabState {
    pub name: String,
    /// Resolved per-tab fetch spec, captured at App::new from the
    /// config so the refresh path doesn't have to re-resolve.
    pub spec: TabSpec,
    pub prs: Vec<PullRequest>,
    pub selected: usize,
    pub last_fetched: Option<std::time::Instant>,
    pub last_error: Option<String>,
}

/// Resolved tab fetch spec — what to send to the bitbucket client.
#[derive(Debug, Clone)]
pub struct TabSpec {
    pub workspace: String,
    /// `None` ⇒ workspace-level lookup (PRs across all repos in the
    /// workspace, scoped by `q`). `Some(repo)` ⇒ single-repo lookup.
    pub repo: Option<String>,
    pub state: String,
    pub q: Option<String>,
}

impl TabSpec {
    /// Resolve a `Tab` config entry against the global default
    /// workspace + the resolved current-user account_id (for `mine`
    /// / `reviewing`). `me_account_id` of `None` is allowed but causes
    /// auto-mode tabs to emit an explanatory error rather than firing
    /// a malformed query.
    pub fn resolve(
        tab: &Tab,
        default_workspace: &str,
        me_account_id: Option<&str>,
    ) -> Result<Self> {
        let workspace = tab
            .workspace
            .clone()
            .unwrap_or_else(|| default_workspace.to_string());
        let (repo, q) = match tab.mode.as_deref() {
            Some("mine") => {
                let aid = me_account_id.ok_or_else(|| {
                    anyhow::anyhow!("mode=\"mine\" needs Account:Read scope on the app password")
                })?;
                let auto = format!("author.account_id = \"{aid}\"");
                let combined = match &tab.q {
                    Some(extra) if !extra.trim().is_empty() => format!("{auto} AND {extra}"),
                    _ => auto,
                };
                (None, Some(combined))
            }
            Some("reviewing") => {
                let aid = me_account_id.ok_or_else(|| {
                    anyhow::anyhow!(
                        "mode=\"reviewing\" needs Account:Read scope on the app password"
                    )
                })?;
                let auto = format!("reviewers.account_id = \"{aid}\"");
                let combined = match &tab.q {
                    Some(extra) if !extra.trim().is_empty() => format!("{auto} AND {extra}"),
                    _ => auto,
                };
                (None, Some(combined))
            }
            None => (tab.repo.clone(), tab.q.clone()),
            Some(other) => {
                return Err(anyhow::anyhow!("unknown tab mode: {other}"));
            }
        };
        Ok(TabSpec {
            workspace,
            repo,
            state: tab.state.clone(),
            q,
        })
    }
}

impl App {
    pub async fn new(cfg: Config, client: Client) -> Result<Self> {
        // Resolve current-user account_id once. Failure is non-fatal
        // — non-auto tabs still work; auto-mode tabs surface the
        // error on their first refresh.
        let (me_account_id, whoami_err) = match client.whoami().await {
            Ok(u) => (u.account_id, None),
            Err(e) => (None, Some(e.to_string())),
        };
        let mut tabs = Vec::with_capacity(cfg.tabs.len());
        for t in &cfg.tabs {
            let spec = match TabSpec::resolve(t, &cfg.workspace, me_account_id.as_deref()) {
                Ok(s) => s,
                Err(e) => {
                    // Park the bad tab with a static error and an
                    // empty spec — switch_tab will still land on it,
                    // refresh will short-circuit on last_error.
                    tabs.push(TabState {
                        name: t.name.clone(),
                        spec: TabSpec {
                            workspace: cfg.workspace.clone(),
                            repo: None,
                            state: t.state.clone(),
                            q: None,
                        },
                        prs: Vec::new(),
                        selected: 0,
                        last_fetched: None,
                        last_error: Some(e.to_string()),
                    });
                    continue;
                }
            };
            tabs.push(TabState {
                name: t.name.clone(),
                spec,
                prs: Vec::new(),
                selected: 0,
                last_fetched: None,
                last_error: None,
            });
        }
        let status = whoami_err
            .as_deref()
            .map(|e| format!("whoami failed: {e}"))
            .unwrap_or_default();
        let mut app = App {
            cfg,
            client,
            me_account_id,
            tabs,
            active_tab: 0,
            status,
            details_visible: false,
            details_scroll: 0,
            detail_cache: HashMap::new(),
            detail_in_flight: None,
        };
        app.refresh_active().await;
        Ok(app)
    }

    /// `(workspace, repo, id)` of the focused PR, or `None` if the
    /// active tab has no PRs. Used as the cache key for the detail
    /// panel.
    pub fn focused_key(&self) -> Option<(String, String, i64)> {
        let tab = self.active();
        let pr = tab.prs.get(tab.selected)?;
        // Bitbucket's `repo_short` returns "workspace/repo"; we split
        // here rather than building a separate accessor since this is
        // the only place we need the parts.
        let full = pr.repo_short();
        let (workspace, repo) = full.split_once('/').unwrap_or(("", full.as_str()));
        Some((workspace.to_string(), repo.to_string(), pr.id))
    }

    /// Toggle the right-half detail panel. Opening lazily fetches the
    /// detail; closing keeps the cache around.
    pub async fn toggle_details(&mut self) {
        self.details_visible = !self.details_visible;
        self.details_scroll = 0;
        if self.details_visible {
            self.ensure_focused_detail().await;
        }
    }

    /// Fetch the detail for the focused PR if not cached and not
    /// in-flight. No-op on tabs with empty PR lists.
    pub async fn ensure_focused_detail(&mut self) {
        let Some(key) = self.focused_key() else {
            return;
        };
        if self.detail_cache.contains_key(&key) || self.detail_in_flight.as_ref() == Some(&key) {
            return;
        }
        self.detail_in_flight = Some(key.clone());
        let (ws, repo, id) = key.clone();
        let pr_res = self.client.get_pr_detail(&ws, &repo, id).await;
        let comments_res = self.client.get_pr_comments(&ws, &repo, id).await;
        self.detail_in_flight = None;
        match (pr_res, comments_res) {
            (Ok(pr), Ok(comments)) => {
                self.detail_cache.insert(key, DetailEntry { pr, comments });
            }
            (Err(e), _) | (_, Err(e)) => {
                self.status = format!("detail fetch failed: {e}");
            }
        }
    }

    /// Drop the cached detail for the focused PR so the next focus
    /// re-fetches. Triggered by `r` while the panel is open.
    pub fn invalidate_focused_detail(&mut self) {
        if let Some(key) = self.focused_key() {
            self.detail_cache.remove(&key);
        }
    }

    /// Approve or unapprove the focused PR, based on the current
    /// state of the cached detail. No-op if the panel isn't open or
    /// we don't have an `account_id` to compare against.
    pub async fn toggle_approval(&mut self) {
        let Some(key) = self.focused_key() else {
            return;
        };
        let Some(me) = self.me_account_id.clone() else {
            self.status = "approve needs Account:Read on the app password".to_string();
            return;
        };
        let approved = self
            .detail_cache
            .get(&key)
            .map(|d| d.pr.approved_by(&me))
            .unwrap_or(false);
        let (ws, repo, id) = key.clone();
        let result = if approved {
            self.client.unapprove_pr(&ws, &repo, id).await
        } else {
            self.client.approve_pr(&ws, &repo, id).await
        };
        match result {
            Ok(()) => {
                self.status = if approved {
                    format!("unapproved {ws}/{repo}#{id}")
                } else {
                    format!("approved {ws}/{repo}#{id}")
                };
                // Drop the cached detail so a fresh fetch picks up
                // the updated participant record + approval count.
                self.detail_cache.remove(&key);
                self.ensure_focused_detail().await;
            }
            Err(e) => {
                self.status = format!("approval toggle failed: {e}");
            }
        }
    }

    pub fn scroll_detail(&mut self, delta: i32) {
        if !self.details_visible {
            return;
        }
        if delta >= 0 {
            self.details_scroll = self.details_scroll.saturating_add(delta as u16);
        } else {
            self.details_scroll = self.details_scroll.saturating_sub((-delta) as u16);
        }
    }

    pub fn active(&self) -> &TabState {
        &self.tabs[self.active_tab]
    }
    pub fn active_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active_tab]
    }

    pub fn switch_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active_tab = idx;
            if self.tabs[idx].last_fetched.is_none() {
                self.status = format!("loading {}…", self.tabs[idx].name);
            }
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.active().prs.len();
        if len == 0 {
            return;
        }
        let s = self.active().selected as isize + delta;
        let new = s.clamp(0, len as isize - 1) as usize;
        self.active_mut().selected = new;
    }

    pub async fn refresh_active(&mut self) {
        let idx = self.active_tab;
        // Bail out for pre-failed tabs (resolution error in App::new).
        if self.tabs[idx].last_error.is_some() && self.tabs[idx].prs.is_empty() {
            self.status = format!(
                "{}: {}",
                self.tabs[idx].name,
                self.tabs[idx].last_error.as_deref().unwrap_or("")
            );
            return;
        }
        let spec = self.tabs[idx].spec.clone();
        let name = self.tabs[idx].name.clone();
        self.status = format!("refreshing {name}…");
        let result = if let Some(repo) = spec.repo.as_deref() {
            self.client
                .list_repo_prs(
                    &spec.workspace,
                    repo,
                    Some(&spec.state),
                    spec.q.as_deref(),
                    50,
                )
                .await
        } else {
            // Workspace-level — pick *any* repo's pullrequests endpoint
            // by spanning the workspace via the `/repositories/<ws>`
            // search isn't directly exposed; instead we issue against
            // a wildcard repo using BBQL's `state` + `author.account_id`
            // / `reviewers.account_id` clauses. Bitbucket exposes this
            // via `/2.0/pullrequests/{selected_user}` for `mine`, but
            // that surface is being deprecated — using per-workspace
            // BBQL keeps both auto-modes uniform.
            //
            // For v0.1 we issue against the default workspace's
            // `/pullrequests` index repo-by-repo only when a repo is
            // set. Without a repo, surface a clear error rather than
            // an empty list.
            self.client
                .list_repo_prs(
                    &spec.workspace,
                    "",
                    Some(&spec.state),
                    spec.q.as_deref(),
                    50,
                )
                .await
        };
        match result {
            Ok(prs) => {
                self.tabs[idx].prs = prs;
                self.tabs[idx].last_fetched = Some(std::time::Instant::now());
                self.tabs[idx].last_error = None;
                self.tabs[idx].selected = self.tabs[idx]
                    .selected
                    .min(self.tabs[idx].prs.len().saturating_sub(1));
                self.status = format!("{} · {} PRs", name, self.tabs[idx].prs.len());
            }
            Err(e) => {
                self.tabs[idx].last_error = Some(e.to_string());
                self.status = format!("error: {e}");
            }
        }
    }

    pub fn open_focused(&mut self) {
        let pr = match self.active().prs.get(self.active().selected) {
            Some(p) => p.clone(),
            None => return,
        };
        let Some(url) = pr.html_url() else {
            self.status = "no html URL on this PR".to_string();
            return;
        };
        let badge = format!("{}#{}", pr.repo_short(), pr.id);
        match webbrowser::open(&url) {
            Ok(()) => self.status = format!("opened {} in browser", badge),
            Err(e) => self.status = format!("open failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Tab;

    fn t(name: &str) -> Tab {
        Tab {
            name: name.into(),
            workspace: None,
            repo: None,
            state: "OPEN".into(),
            mode: None,
            q: None,
        }
    }

    #[test]
    fn resolve_repo_tab_uses_default_workspace() {
        let mut tab = t("repo");
        tab.repo = Some("tattle-api".into());
        let spec = TabSpec::resolve(&tab, "tattlecorp", None).unwrap();
        assert_eq!(spec.workspace, "tattlecorp");
        assert_eq!(spec.repo.as_deref(), Some("tattle-api"));
        assert_eq!(spec.state, "OPEN");
        assert!(spec.q.is_none());
    }

    #[test]
    fn resolve_tab_workspace_overrides_default() {
        let mut tab = t("repo");
        tab.workspace = Some("otherws".into());
        tab.repo = Some("repoA".into());
        let spec = TabSpec::resolve(&tab, "default", None).unwrap();
        assert_eq!(spec.workspace, "otherws");
    }

    #[test]
    fn resolve_mine_builds_author_bbql() {
        let mut tab = t("mine");
        tab.mode = Some("mine".into());
        let spec = TabSpec::resolve(&tab, "ws", Some("aid:abc")).unwrap();
        let q = spec.q.unwrap();
        assert!(q.contains("author.account_id = \"aid:abc\""));
    }

    #[test]
    fn resolve_reviewing_builds_reviewer_bbql() {
        let mut tab = t("rev");
        tab.mode = Some("reviewing".into());
        let spec = TabSpec::resolve(&tab, "ws", Some("aid:abc")).unwrap();
        let q = spec.q.unwrap();
        assert!(q.contains("reviewers.account_id = \"aid:abc\""));
    }

    #[test]
    fn resolve_mine_without_account_id_errors() {
        let mut tab = t("mine");
        tab.mode = Some("mine".into());
        let err = TabSpec::resolve(&tab, "ws", None).unwrap_err();
        assert!(err.to_string().contains("Account:Read"));
    }

    #[test]
    fn resolve_mine_with_extra_q_appends_with_and() {
        let mut tab = t("mine");
        tab.mode = Some("mine".into());
        tab.q = Some("state != \"DECLINED\"".into());
        let spec = TabSpec::resolve(&tab, "ws", Some("aid:abc")).unwrap();
        let q = spec.q.unwrap();
        assert!(q.contains(" AND "));
    }
}
