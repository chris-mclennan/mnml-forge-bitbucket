//! ratatui rendering + the main event loop.

use crate::app::App;
use crate::keys;
use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Tabs},
};
use std::io::Stdout;
use std::time::{Duration, Instant};

pub async fn run(app: &mut App) -> Result<()> {
    let mut stdout = std::io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = event_loop(&mut terminal, app).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|f| draw(f, app))?;
        if app.cfg.refresh_interval_secs > 0
            && last_refresh.elapsed().as_secs() >= app.cfg.refresh_interval_secs
        {
            app.refresh_active().await;
            last_refresh = Instant::now();
        }
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == event::KeyEventKind::Press => {
                    if let Some(action) = keys::handle(key, app) {
                        let quit = keys::apply(action, app).await;
                        if quit {
                            break;
                        }
                        last_refresh = Instant::now();
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}

pub fn draw(f: &mut Frame, app: &App) {
    let size = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(size);
    draw_tabs(f, chunks[0], app);
    if app.details_visible {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(chunks[1]);
        draw_table(f, body[0], app);
        draw_detail(f, body[1], app);
    } else {
        draw_table(f, chunks[1], app);
    }
    draw_status(f, chunks[2], app);
}

fn draw_detail(f: &mut Frame, area: Rect, app: &App) {
    let Some(key) = app.focused_key() else {
        let p = Paragraph::new("(no PR focused)")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL).title(" detail "));
        f.render_widget(p, area);
        return;
    };
    let entry = match app.detail_cache.get(&key) {
        Some(e) => e,
        None => {
            let msg = if app.detail_in_flight.as_ref() == Some(&key) {
                "loading detail…"
            } else {
                "(no detail cached — press d to refresh)"
            };
            let p = Paragraph::new(msg)
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title(" detail "));
            f.render_widget(p, area);
            return;
        }
    };
    let pr = &entry.pr;
    let (ws, repo, id) = (&key.0, &key.1, key.2);
    let me_approved = app
        .me_account_id
        .as_deref()
        .map(|m| pr.approved_by(m))
        .unwrap_or(false);
    let approval_chip = if me_approved {
        Span::styled(
            format!("✓ you approved · {} total", pr.approval_count()),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("○ not approved · {} total", pr.approval_count()),
            Style::default().fg(Color::Yellow),
        )
    };
    let title = format!(" {ws}/{repo}#{id} ");

    let header_lines = vec![
        Line::from(vec![
            Span::styled(
                pr.state.clone(),
                Style::default().fg(state_color(&pr.state)),
            ),
            Span::raw(" · "),
            Span::raw(format!(
                "{} → {}",
                pr.source
                    .as_ref()
                    .and_then(|b| b.branch.as_ref().map(|n| n.name.clone()))
                    .unwrap_or_else(|| "?".into()),
                pr.destination
                    .as_ref()
                    .and_then(|b| b.branch.as_ref().map(|n| n.name.clone()))
                    .unwrap_or_else(|| "?".into()),
            )),
        ]),
        Line::from(format!(
            "author: {} · updated: {}",
            pr.author
                .as_ref()
                .map(|u| u.display_name.as_str())
                .unwrap_or("—"),
            pr.updated_date()
        )),
        Line::from(approval_chip),
        Line::from(""),
        Line::from(Span::styled(
            pr.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    let mut body: Vec<Line> = header_lines;
    if let Some(desc) = &pr.description
        && !desc.raw.trim().is_empty()
    {
        for line in desc.raw.lines() {
            body.push(Line::from(line.to_string()));
        }
        body.push(Line::from(""));
    } else {
        body.push(Line::from(Span::styled(
            "(no description)",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));
        body.push(Line::from(""));
    }

    body.push(Line::from(Span::styled(
        format!("comments ({}, most-recent first):", entry.comments.len()),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )));
    body.push(Line::from(""));

    // Bitbucket returns comments oldest-first; reverse so the detail
    // panel matches the jira viewer's most-recent-first convention.
    for c in entry.comments.iter().rev().take(20) {
        let head = Line::from(vec![
            Span::styled(
                format!("  {} · ", c.author()),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(c.created_date(), Style::default().fg(Color::DarkGray)),
        ]);
        body.push(head);
        for line in c.body().lines() {
            body.push(Line::from(format!("    {line}")));
        }
        body.push(Line::from(""));
    }

    let block = Block::default().borders(Borders::ALL).title(title);
    let p = Paragraph::new(body)
        .block(block)
        .scroll((app.details_scroll, 0))
        .wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(p, area);
}

fn state_color(state: &str) -> Color {
    match state {
        "OPEN" => Color::Green,
        "MERGED" => Color::Magenta,
        "DECLINED" => Color::Red,
        "SUPERSEDED" => Color::DarkGray,
        _ => Color::Gray,
    }
}

fn draw_tabs(f: &mut Frame, area: Rect, app: &App) {
    let labels: Vec<Line> = app
        .tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let n = t.data.len();
            let label = if t.last_fetched.is_some() {
                format!("{}.{} ({n})", i + 1, t.name)
            } else {
                format!("{}.{}", i + 1, t.name)
            };
            Line::from(label)
        })
        .collect();
    let tabs = Tabs::new(labels)
        .block(Block::default().borders(Borders::ALL).title(" bitbucket "))
        .select(app.active_tab)
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

fn draw_table(f: &mut Frame, area: Rect, app: &App) {
    let tab = app.active();
    if let Some(err) = &tab.last_error {
        let p = Paragraph::new(format!("error: {err}\n\nPress `r` to retry."))
            .style(Style::default().fg(Color::Red))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", tab.name)),
            );
        f.render_widget(p, area);
        return;
    }
    if tab.data.is_empty() && tab.last_fetched.is_some() {
        let p = Paragraph::new(empty_message(tab.spec.kind))
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", tab.name)),
            );
        f.render_widget(p, area);
        return;
    }
    if tab.data.is_empty() {
        let p = Paragraph::new("loading…")
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", tab.name)),
            );
        f.render_widget(p, area);
        return;
    }
    match &tab.data {
        crate::app::TabData::PullRequests(prs) => draw_pr_table(f, area, tab, prs),
        crate::app::TabData::Pipelines(ps) => draw_pipeline_table(f, area, tab, ps),
        crate::app::TabData::Branches(bs) => draw_branch_table(f, area, tab, bs),
    }
}

fn empty_message(kind: crate::app::TabKind) -> &'static str {
    match kind {
        crate::app::TabKind::PullRequests => "(no PRs match this tab)",
        crate::app::TabKind::Pipelines => "(no pipelines have run on this repo)",
        crate::app::TabKind::Branches => "(no branches in this repo)",
    }
}

fn draw_pr_table(
    f: &mut Frame,
    area: Rect,
    tab: &crate::app::TabState,
    prs: &[crate::bitbucket::PullRequest],
) {
    let header = Row::new(vec![
        Cell::from("REPO"),
        Cell::from("PR"),
        Cell::from("STATE"),
        Cell::from("AUTHOR"),
        Cell::from("BRANCH → DEST"),
        Cell::from("UPDATED"),
        Cell::from("TITLE"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let rows: Vec<Row> = prs
        .iter()
        .map(|p| {
            let repo = p.repo_short();
            let key = format!("#{}", p.id);
            let state = p.state.clone();
            let state_style = Style::default().fg(state_color(&state));
            let author = p
                .author
                .as_ref()
                .map(|u| u.display_name.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "—".to_string());
            let branches = format!(
                "{} → {}",
                p.source
                    .as_ref()
                    .and_then(|b| b.branch.as_ref().map(|n| n.name.clone()))
                    .unwrap_or_else(|| "?".into()),
                p.destination
                    .as_ref()
                    .and_then(|b| b.branch.as_ref().map(|n| n.name.clone()))
                    .unwrap_or_else(|| "?".into()),
            );
            let updated = p.updated_date();
            Row::new(vec![
                Cell::from(repo),
                Cell::from(key).style(Style::default().fg(Color::Yellow)),
                Cell::from(state).style(state_style),
                Cell::from(author),
                Cell::from(branches),
                Cell::from(updated),
                Cell::from(p.title.clone()),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(24),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(16),
        Constraint::Length(28),
        Constraint::Length(12),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", tab.name)),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut state = TableState::default();
    state.select(Some(tab.selected));
    f.render_stateful_widget(table, area, &mut state);
}

fn draw_pipeline_table(
    f: &mut Frame,
    area: Rect,
    tab: &crate::app::TabState,
    ps: &[crate::bitbucket::Pipeline],
) {
    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("STATE"),
        Cell::from("BRANCH"),
        Cell::from("COMMIT"),
        Cell::from("TRIGGER"),
        Cell::from("DURATION"),
        Cell::from("CREATED"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let rows: Vec<Row> = ps
        .iter()
        .map(|p| {
            let state = p.state_label();
            let state_style = Style::default().fg(pipeline_state_color(&state));
            Row::new(vec![
                Cell::from(format!("#{}", p.build_number))
                    .style(Style::default().fg(Color::Yellow)),
                Cell::from(state).style(state_style),
                Cell::from(p.branch_label()),
                Cell::from(p.short_sha()),
                Cell::from(p.trigger_label()),
                Cell::from(p.duration_label()),
                Cell::from(p.created_date()),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(24),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(12),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", tab.name)),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut state = TableState::default();
    state.select(Some(tab.selected));
    f.render_stateful_widget(table, area, &mut state);
}

fn draw_branch_table(
    f: &mut Frame,
    area: Rect,
    tab: &crate::app::TabState,
    bs: &[crate::bitbucket::BranchRef],
) {
    let header = Row::new(vec![
        Cell::from("BRANCH"),
        Cell::from("COMMIT"),
        Cell::from("LATEST"),
        Cell::from("AUTHOR"),
        Cell::from("MESSAGE"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let rows: Vec<Row> = bs
        .iter()
        .map(|b| {
            Row::new(vec![
                Cell::from(b.name.clone()),
                Cell::from(b.short_sha()).style(Style::default().fg(Color::Yellow)),
                Cell::from(b.latest_date()),
                Cell::from(b.author_label()),
                Cell::from(b.summary_line()),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(32),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(20),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", tab.name)),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut state = TableState::default();
    state.select(Some(tab.selected));
    f.render_stateful_widget(table, area, &mut state);
}

fn pipeline_state_color(state: &str) -> Color {
    match state {
        "SUCCESSFUL" => Color::Green,
        "FAILED" | "ERROR" => Color::Red,
        "STOPPED" | "HALTED" => Color::DarkGray,
        "IN_PROGRESS" | "PENDING" | "RUNNING" => Color::Yellow,
        _ => Color::Gray,
    }
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let hint = " 1-9 tab · ↑↓/jk move · Enter/o open · d detail · a approve · r refresh · q quit ";
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.status),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            hint,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}
