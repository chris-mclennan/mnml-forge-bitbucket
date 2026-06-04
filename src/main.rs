mod app;
mod auth;
mod bitbucket;
mod blit;
mod config;
mod keys;
mod ui;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "mnml-tickets-bitbucket",
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
    /// a pane (`:host.launch mnml-tickets-bitbucket`).
    #[arg(long, value_name = "SOCKET")]
    blit: Option<String>,
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
