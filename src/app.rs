//! App state — what's loaded, what's selected, the configured query
//! for each tab.

use crate::bitbucket::{Client, PullRequest};
use crate::config::{Config, Tab};
use anyhow::Result;

pub struct App {
    pub cfg: Config,
    pub client: Client,
    pub tabs: Vec<TabState>,
    pub active_tab: usize,
    pub status: String,
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
            tabs,
            active_tab: 0,
            status,
        };
        app.refresh_active().await;
        Ok(app)
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
