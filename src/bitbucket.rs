//! Minimal Bitbucket Cloud REST API v2 client for pull requests.
//!
//! Base URL: https://api.bitbucket.org/2.0
//! Auth: HTTP Basic with `<email>:<app-password>`. App passwords are
//!       configured at <https://bitbucket.org/account/settings/app-passwords/>
//!       and must have at least `Pull requests: Read` scope.
//! Docs: https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/

use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Deserialize;

const BASE: &str = "https://api.bitbucket.org/2.0";

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    auth_header: String,
}

impl Client {
    pub fn new(email: &str, app_password: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "mnml-tickets-bitbucket/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()?;
        let basic = B64.encode(format!("{email}:{app_password}"));
        Ok(Self {
            http,
            auth_header: format!("Basic {basic}"),
        })
    }

    /// Pull requests for a single repo. `state` is one of
    /// `OPEN` / `MERGED` / `DECLINED` / `SUPERSEDED`. `q` is an
    /// optional Bitbucket Query Language string layered on top
    /// (e.g. `author.account_id = "{...}"`).
    pub async fn list_repo_prs(
        &self,
        workspace: &str,
        repo: &str,
        state: Option<&str>,
        q: Option<&str>,
        per_page: u32,
    ) -> Result<Vec<PullRequest>> {
        let url = format!("{BASE}/repositories/{workspace}/{repo}/pullrequests");
        // Bitbucket Cloud supports `state` as a query param, repeated
        // for OR — v0.1 takes a single state at a time.
        let mut req = self
            .http
            .get(&url)
            .header("Authorization", &self.auth_header)
            .header("Accept", "application/json")
            .query(&[("pagelen", per_page.to_string())]);
        if let Some(s) = state {
            req = req.query(&[("state", s)]);
        }
        if let Some(query) = q {
            req = req.query(&[("q", query)]);
        }
        let resp = req.send().await.context("bitbucket PR list failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("bitbucket {status}: {text}"));
        }
        let body: PrPage = resp
            .json()
            .await
            .context("parsing bitbucket PR list response")?;
        Ok(body.values)
    }

    /// `GET /user` — returns the authenticated user. Used by --check
    /// + to resolve `mode = "mine"` / `mode = "reviewing"` tabs.
    pub async fn whoami(&self) -> Result<AuthUser> {
        let resp = self
            .http
            .get(format!("{BASE}/user"))
            .header("Authorization", &self.auth_header)
            .header("Accept", "application/json")
            .send()
            .await
            .context("bitbucket whoami failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("bitbucket whoami: {status}: {text}"));
        }
        resp.json().await.context("parsing bitbucket whoami")
    }
}

#[derive(Debug, Deserialize)]
struct PrPage {
    #[serde(default)]
    values: Vec<PullRequest>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct PullRequest {
    pub id: i64,
    pub title: String,
    pub state: String,
    /// ISO 8601 with timezone — we keep the raw string and slice for
    /// the date in the table (saves a chrono parse on the render path).
    #[serde(default)]
    pub updated_on: Option<String>,
    #[serde(default)]
    pub author: Option<User>,
    #[serde(default)]
    pub destination: Option<Branch>,
    #[serde(default)]
    pub source: Option<Branch>,
    #[serde(default)]
    pub links: Option<Links>,
    /// Optional summary list. May be absent on lists; present on detail.
    #[serde(default)]
    pub reviewers: Vec<User>,
}

impl PullRequest {
    /// Returns the public HTML URL, falling back to a deterministic
    /// `bitbucket.org/<ws>/<repo>/pull-requests/<id>` if links are
    /// missing.
    pub fn html_url(&self) -> Option<String> {
        self.links
            .as_ref()
            .and_then(|l| l.html.as_ref())
            .map(|h| h.href.clone())
    }

    /// `<workspace>/<repo>` derived from the source/destination
    /// branch's repository link (Bitbucket nests `repository` under
    /// `source` and `destination`). Falls back to an empty string.
    pub fn repo_short(&self) -> String {
        if let Some(b) = self.destination.as_ref().or(self.source.as_ref())
            && let Some(r) = b.repository.as_ref()
        {
            return r.full_name.clone();
        }
        String::new()
    }

    /// Just the date portion of `updated_on` (`YYYY-MM-DD`).
    pub fn updated_date(&self) -> String {
        self.updated_on
            .as_deref()
            .map(|s| s.chars().take(10).collect::<String>())
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
#[allow(dead_code)]
pub struct User {
    #[serde(default)]
    pub display_name: String,
    /// `account_id` is the stable identifier used by BBQL. v0.1
    /// doesn't dispatch on it but auth-mode resolution will.
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[allow(dead_code)]
pub struct Branch {
    #[serde(default)]
    pub branch: Option<BranchName>,
    #[serde(default)]
    pub repository: Option<Repo>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[allow(dead_code)]
pub struct BranchName {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[allow(dead_code)]
pub struct Repo {
    #[serde(default)]
    pub full_name: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Links {
    #[serde(default)]
    pub html: Option<HrefLink>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct HrefLink {
    #[serde(default)]
    pub href: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AuthUser {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(state: &str) -> PullRequest {
        PullRequest {
            id: 42,
            title: "Fix bufferline crash".into(),
            state: state.into(),
            updated_on: Some("2026-06-04T10:23:11.000+0000".into()),
            author: Some(User {
                display_name: "alice".into(),
                account_id: Some("aid:abc".into()),
            }),
            destination: Some(Branch {
                branch: Some(BranchName {
                    name: "main".into(),
                }),
                repository: Some(Repo {
                    full_name: "tattlecorp/tattle-api".into(),
                }),
            }),
            source: Some(Branch {
                branch: Some(BranchName {
                    name: "alice/fix".into(),
                }),
                repository: Some(Repo {
                    full_name: "tattlecorp/tattle-api".into(),
                }),
            }),
            links: Some(Links {
                html: Some(HrefLink {
                    href: "https://bitbucket.org/tattlecorp/tattle-api/pull-requests/42".into(),
                }),
            }),
            reviewers: vec![],
        }
    }

    #[test]
    fn repo_short_returns_destination_full_name() {
        assert_eq!(pr("OPEN").repo_short(), "tattlecorp/tattle-api");
    }

    #[test]
    fn updated_date_takes_first_ten_chars() {
        assert_eq!(pr("OPEN").updated_date(), "2026-06-04");
    }

    #[test]
    fn html_url_pulls_from_links() {
        assert_eq!(
            pr("MERGED").html_url(),
            Some("https://bitbucket.org/tattlecorp/tattle-api/pull-requests/42".into())
        );
    }

    #[test]
    fn html_url_is_none_when_links_missing() {
        let mut p = pr("OPEN");
        p.links = None;
        assert_eq!(p.html_url(), None);
    }
}
