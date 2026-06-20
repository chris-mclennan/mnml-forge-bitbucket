mod app;
mod auth;
mod bitbucket;
mod blit;
mod clipboard;
mod config;
mod headless;
mod keys;
mod theme;
mod ui;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "mnml-forge-bitbucket",
    version,
    about = "Bitbucket Cloud PR viewer for mnml"
)]
struct Cli {
    /// Print the resolved config + auth state and exit. Hits the API
    /// to verify the app password works (`/2.0/user`).
    #[arg(long)]
    check: bool,
    /// Blit-host mode — render into a UDS-served cell grid instead of
    /// the local terminal. Used by mnml / tmnl to host this binary as
    /// a pane (`:host.launch mnml-forge-bitbucket`).
    #[arg(long, value_name = "SOCKET")]
    blit: Option<String>,
    /// Headless: print every open PR the configured PR tabs would
    /// surface, as JSON on stdout, then exit. Used by mnml's
    /// `pr.picker` cross-host palette command and by the rail's
    /// "Open PRs" subsection refresh. Requires `--json` (only
    /// shape supported v1).
    #[arg(long)]
    list_prs: bool,
    /// Headless: print the URL of the most recent pipeline run on
    /// `--branch` in `--owner/--repo`, as `{"url": "..."}` JSON on
    /// stdout. Used by mnml's pr.picker Tab → cross-nav. Returns
    /// `{"url": null}` when no matching pipeline is found.
    #[arg(long)]
    find_pipeline_for_pr: bool,
    /// Owner (workspace) for `--find-pipeline-for-pr`.
    #[arg(long)]
    owner: Option<String>,
    /// Repo for `--find-pipeline-for-pr`.
    #[arg(long)]
    repo: Option<String>,
    /// Source branch name for `--find-pipeline-for-pr`.
    #[arg(long)]
    branch: Option<String>,
    /// Required for `--list-prs` / `--find-pipeline-for-pr`. Reserves
    /// the headless surface for future shapes.
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Config first so first-run users get the scaffold-template
    // path before being asked for an app password.
    let cfg = config::load()?;
    let token = auth::load_token().with_context(|| {
        format!(
            "couldn't load app password from {}",
            auth::token_path().display()
        )
    })?;
    let client = bitbucket::Client::new(&cfg.email, &token)?;

    if cli.list_prs {
        if !cli.json {
            anyhow::bail!("--list-prs requires --json (only shape supported v1)");
        }
        return headless::list_prs(&cfg, &client).await;
    }

    if cli.find_pipeline_for_pr {
        if !cli.json {
            anyhow::bail!("--find-pipeline-for-pr requires --json");
        }
        let owner = cli
            .owner
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--owner is required"))?;
        let repo = cli
            .repo
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--repo is required"))?;
        let branch = cli
            .branch
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--branch is required"))?;
        return headless::find_pipeline_for_pr(&client, owner, repo, branch).await;
    }

    if cli.check {
        println!("config: {}", config::config_path().display());
        println!(
            "token: {} (loaded, {} chars)",
            auth::token_path().display(),
            token.len()
        );
        println!("workspace: {}", cfg.workspace);
        println!("email: {}", cfg.email);
        println!("refresh_interval_secs: {}", cfg.refresh_interval_secs);
        match client.whoami().await {
            Ok(u) => println!(
                "whoami: ok — {} (account_id: {})",
                u.display_name,
                u.account_id.as_deref().unwrap_or("<none>")
            ),
            Err(e) => println!("whoami: FAIL — {e}"),
        }
        for (i, t) in cfg.tabs.iter().enumerate() {
            let shape = match (&t.mode, &t.repo, &t.q) {
                (Some(m), _, _) => format!("mode={m}"),
                (None, Some(r), _) => format!("repo={r}"),
                (None, None, Some(_)) => "q=<custom>".to_string(),
                _ => "<invalid>".to_string(),
            };
            println!("  tab {} ({}): {shape}, state={}", i + 1, t.name, t.state);
        }
        return Ok(());
    }

    let mut app = app::App::new(cfg, client).await?;

    if let Some(socket) = cli.blit {
        blit::run(&mut app, std::path::Path::new(&socket)).await
    } else {
        ui::run(&mut app).await
    }
}
