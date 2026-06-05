//! App state — what's loaded, what's selected, the configured query
//! for each tab.

use crate::bitbucket::{BranchRef, Client, Comment, Pipeline, PullRequest};
use crate::config::{Config, Tab};
use anyhow::Result;
use std::collections::HashMap;

/// Per-tab content kind. The PR / Pipeline / Branch dispatch lives on
/// `TabKind` rather than a bare string so the refresh + render paths
/// can exhaustively match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    PullRequests,
    Pipelines,
    Branches,
}

impl TabKind {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "pull_requests" => Ok(Self::PullRequests),
            "pipelines" => Ok(Self::Pipelines),
            "branches" => Ok(Self::Branches),
            other => Err(anyhow::anyhow!("unknown tab kind: {other}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PullRequests => "pull_requests",
            Self::Pipelines => "pipelines",
            Self::Branches => "branches",
        }
    }
}

/// Loaded data for a tab — variant determined by the resolved `TabKind`.
#[derive(Debug, Clone)]
pub enum TabData {
    PullRequests(Vec<PullRequest>),
    Pipelines(Vec<Pipeline>),
    Branches(Vec<BranchRef>),
}

impl TabData {
    pub fn empty_for(kind: TabKind) -> Self {
        match kind {
            TabKind::PullRequests => Self::PullRequests(Vec::new()),
            TabKind::Pipelines => Self::Pipelines(Vec::new()),
            TabKind::Branches => Self::Branches(Vec::new()),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::PullRequests(v) => v.len(),
            Self::Pipelines(v) => v.len(),
            Self::Branches(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

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
    /// Only meaningful on `PullRequests` tabs in v0.3 — other kinds
    /// render a brief "no detail panel for this view" message.
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
    pub data: TabData,
    pub selected: usize,
    pub last_fetched: Option<std::time::Instant>,
    pub last_error: Option<String>,
}

/// Resolved tab fetch spec — what to send to the bitbucket client.
#[derive(Debug, Clone)]
pub struct TabSpec {
    pub kind: TabKind,
    pub workspace: String,
    /// `None` ⇒ workspace-level lookup (PRs across all repos in the
    /// workspace, scoped by `q`). `Some(repo)` ⇒ single-repo lookup.
    /// Pipelines + Branches require Some(repo).
    pub repo: Option<String>,
    /// PR state — only meaningful for `kind = PullRequests`.
    pub state: String,
    /// BBQL — only meaningful for `kind = PullRequests`.
    pub q: Option<String>,
}

impl TabSpec {
    /// Resolve a `Tab` config entry against the global default
    /// workspace + the resolved current-user account_id (for `mine`
    /// / `reviewing`). `me_account_id` of `None` is allowed but causes
    /// auto-mode PR tabs to emit an explanatory error rather than
    /// firing a malformed query.
    pub fn resolve(
        tab: &Tab,
        default_workspace: &str,
        me_account_id: Option<&str>,
    ) -> Result<Self> {
        let kind = TabKind::from_str(&tab.kind)?;
        let workspace = tab
            .workspace
            .clone()
            .unwrap_or_else(|| default_workspace.to_string());
        match kind {
            TabKind::PullRequests => {
                let (repo, q) = match tab.mode.as_deref() {
                    Some("mine") => {
                        let aid = me_account_id.ok_or_else(|| {
                            anyhow::anyhow!(
                                "mode=\"mine\" needs Account:Read scope on the app password"
                            )
                        })?;
                        let auto = format!("author.account_id = \"{aid}\"");
                        let combined = match &tab.q {
                            Some(extra) if !extra.trim().is_empty() => {
                                format!("{auto} AND {extra}")
                            }
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
                            Some(extra) if !extra.trim().is_empty() => {
                                format!("{auto} AND {extra}")
                            }
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
                    kind,
                    workspace,
                    repo,
                    state: tab.state.clone(),
                    q,
                })
            }
            TabKind::Pipelines | TabKind::Branches => {
                let repo = tab.repo.clone().ok_or_else(|| {
                    anyhow::anyhow!("kind = `{}` requires a `repo` field", kind.as_str())
                })?;
                Ok(TabSpec {
                    kind,
                    workspace,
                    repo: Some(repo),
                    state: String::new(),
                    q: None,
                })
            }
        }
    }
}

impl App {
    pub async fn new(cfg: Config, client: Client) -> Result<Self> {
        // Resolve current-user account_id once. Failure is non-fatal
        // — non-auto tabs still work; auto-mode PR tabs surface the
        // error on their first refresh.
        let (me_account_id, whoami_err) = match client.whoami().await {
            Ok(u) => (u.account_id, None),
            Err(e) => (None, Some(e.to_string())),
        };
        let mut tabs = Vec::with_capacity(cfg.tabs.len());
        for t in &cfg.tabs {
            let parsed_kind = TabKind::from_str(&t.kind).unwrap_or(TabKind::PullRequests);
            match TabSpec::resolve(t, &cfg.workspace, me_account_id.as_deref()) {
                Ok(spec) => tabs.push(TabState {
                    name: t.name.clone(),
                    data: TabData::empty_for(spec.kind),
                    spec,
                    selected: 0,
                    last_fetched: None,
                    last_error: None,
                }),
                Err(e) => tabs.push(TabState {
                    name: t.name.clone(),
                    spec: TabSpec {
                        kind: parsed_kind,
                        workspace: cfg.workspace.clone(),
                        repo: None,
                        state: t.state.clone(),
                        q: None,
                    },
                    data: TabData::empty_for(parsed_kind),
                    selected: 0,
                    last_fetched: None,
                    last_error: Some(e.to_string()),
                }),
            }
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
        let len = self.active().data.len();
        if len == 0 {
            return;
        }
        let s = self.active().selected as isize + delta;
        let new = s.clamp(0, len as isize - 1) as usize;
        self.active_mut().selected = new;
    }

    /// `(workspace, repo, id)` of the focused PR, or `None` if the
    /// active tab isn't a PR tab or has no rows. Used as the detail
    /// cache key.
    pub fn focused_key(&self) -> Option<(String, String, i64)> {
        let tab = self.active();
        let TabData::PullRequests(prs) = &tab.data else {
            return None;
        };
        let pr = prs.get(tab.selected)?;
        let full = pr.repo_short();
        let (workspace, repo) = full.split_once('/').unwrap_or(("", full.as_str()));
        Some((workspace.to_string(), repo.to_string(), pr.id))
    }

    pub async fn refresh_active(&mut self) {
        let idx = self.active_tab;
        // Bail out for pre-failed tabs (resolution error in App::new).
        if self.tabs[idx].last_error.is_some() && self.tabs[idx].data.is_empty() {
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
        match spec.kind {
            TabKind::PullRequests => {
                let result = self
                    .client
                    .list_repo_prs(
                        &spec.workspace,
                        spec.repo.as_deref().unwrap_or(""),
                        Some(&spec.state),
                        spec.q.as_deref(),
                        50,
                    )
                    .await;
                self.commit_pr_refresh(idx, name, result);
            }
            TabKind::Pipelines => {
                let repo = spec.repo.as_deref().unwrap_or("");
                let result = self.client.list_pipelines(&spec.workspace, repo, 50).await;
                self.commit_pipeline_refresh(idx, name, result);
            }
            TabKind::Branches => {
                let repo = spec.repo.as_deref().unwrap_or("");
                let result = self.client.list_branches(&spec.workspace, repo, 50).await;
                self.commit_branch_refresh(idx, name, result);
            }
        }
    }

    fn commit_pr_refresh(&mut self, idx: usize, name: String, result: Result<Vec<PullRequest>>) {
        match result {
            Ok(prs) => {
                let n = prs.len();
                self.tabs[idx].data = TabData::PullRequests(prs);
                self.tabs[idx].last_fetched = Some(std::time::Instant::now());
                self.tabs[idx].last_error = None;
                self.tabs[idx].selected = self.tabs[idx].selected.min(n.saturating_sub(1));
                self.status = format!("{name} · {n} PRs");
            }
            Err(e) => {
                self.tabs[idx].last_error = Some(e.to_string());
                self.status = format!("error: {e}");
            }
        }
    }

    fn commit_pipeline_refresh(&mut self, idx: usize, name: String, result: Result<Vec<Pipeline>>) {
        match result {
            Ok(ps) => {
                let n = ps.len();
                self.tabs[idx].data = TabData::Pipelines(ps);
                self.tabs[idx].last_fetched = Some(std::time::Instant::now());
                self.tabs[idx].last_error = None;
                self.tabs[idx].selected = self.tabs[idx].selected.min(n.saturating_sub(1));
                self.status = format!("{name} · {n} pipelines");
            }
            Err(e) => {
                self.tabs[idx].last_error = Some(e.to_string());
                self.status = format!("error: {e}");
            }
        }
    }

    fn commit_branch_refresh(&mut self, idx: usize, name: String, result: Result<Vec<BranchRef>>) {
        match result {
            Ok(bs) => {
                let n = bs.len();
                self.tabs[idx].data = TabData::Branches(bs);
                self.tabs[idx].last_fetched = Some(std::time::Instant::now());
                self.tabs[idx].last_error = None;
                self.tabs[idx].selected = self.tabs[idx].selected.min(n.saturating_sub(1));
                self.status = format!("{name} · {n} branches");
            }
            Err(e) => {
                self.tabs[idx].last_error = Some(e.to_string());
                self.status = format!("error: {e}");
            }
        }
    }

    /// Open whatever the focused row points at in the browser.
    /// Per-kind URL strategy:
    ///   - PR: `pr.html_url()` (Bitbucket sends one in `links.html`)
    ///   - Pipeline: bitbucket.org/<ws>/<repo>/pipelines/results/<n>
    ///   - Branch: bitbucket.org/<ws>/<repo>/branch/<name>
    pub fn open_focused(&mut self) {
        let tab = self.active();
        let workspace = tab.spec.workspace.clone();
        let repo = tab.spec.repo.clone().unwrap_or_default();
        let url = match &tab.data {
            TabData::PullRequests(prs) => match prs.get(tab.selected) {
                Some(p) => p.html_url(),
                None => return,
            },
            TabData::Pipelines(ps) => ps.get(tab.selected).map(|p| {
                format!(
                    "https://bitbucket.org/{workspace}/{repo}/pipelines/results/{}",
                    p.build_number
                )
            }),
            TabData::Branches(bs) => bs
                .get(tab.selected)
                .map(|b| format!("https://bitbucket.org/{workspace}/{repo}/branch/{}", b.name)),
        };
        let Some(url) = url else {
            self.status = "no URL for this row".to_string();
            return;
        };
        match webbrowser::open(&url) {
            Ok(()) => self.status = format!("opened {url}"),
            Err(e) => self.status = format!("open failed: {e}"),
        }
    }

    /// Toggle the right-half detail panel. Opening lazily fetches the
    /// detail; closing keeps the cache around. No-op on non-PR tabs.
    pub async fn toggle_details(&mut self) {
        if self.active().spec.kind != TabKind::PullRequests {
            self.status = "detail panel is PR-only in v0.3".to_string();
            return;
        }
        self.details_visible = !self.details_visible;
        self.details_scroll = 0;
        if self.details_visible {
            self.ensure_focused_detail().await;
        }
    }

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

    pub fn invalidate_focused_detail(&mut self) {
        if let Some(key) = self.focused_key() {
            self.detail_cache.remove(&key);
        }
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Tab;

    fn t(name: &str) -> Tab {
        Tab {
            name: name.into(),
            kind: "pull_requests".into(),
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
        assert_eq!(spec.kind, TabKind::PullRequests);
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

    #[test]
    fn resolve_pipelines_kind_requires_repo() {
        let mut tab = t("p");
        tab.kind = "pipelines".into();
        let err = TabSpec::resolve(&tab, "ws", None).unwrap_err();
        assert!(err.to_string().contains("repo"));
    }

    #[test]
    fn resolve_pipelines_kind_with_repo_succeeds() {
        let mut tab = t("p");
        tab.kind = "pipelines".into();
        tab.repo = Some("myrepo".into());
        let spec = TabSpec::resolve(&tab, "ws", None).unwrap();
        assert_eq!(spec.kind, TabKind::Pipelines);
        assert_eq!(spec.repo.as_deref(), Some("myrepo"));
    }

    #[test]
    fn resolve_branches_kind_requires_repo() {
        let mut tab = t("b");
        tab.kind = "branches".into();
        let err = TabSpec::resolve(&tab, "ws", None).unwrap_err();
        assert!(err.to_string().contains("repo"));
    }

    #[test]
    fn resolve_unknown_kind_errors() {
        let mut tab = t("bad");
        tab.kind = "garbage".into();
        let err = TabSpec::resolve(&tab, "ws", None).unwrap_err();
        assert!(err.to_string().contains("garbage"));
    }
}
