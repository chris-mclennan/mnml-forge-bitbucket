//! Keyboard chord → action mapping.

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub enum Action {
    Quit,
    Refresh,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    OpenInBrowser,
    /// `y` — copy the focused row's URL to the OS clipboard.
    /// Restores the pre-split `bitbucket.copy_selected_pr_url`
    /// / `bitbucket.copy_selected_url` palette commands.
    YankUrl,
    SwitchTab(usize),
    NextTab,
    PrevTab,
    ToggleDetails,
    DetailScrollUp,
    DetailScrollDown,
    ToggleApproval,
}

pub fn handle(key: KeyEvent, _app: &App) -> Option<Action> {
    let m = key.modifiers;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
        KeyCode::Char('c') if ctrl => Some(Action::Quit),
        KeyCode::Char('r') => Some(Action::Refresh),
        // `Ctrl+U` / `Ctrl+D` scroll the detail pane when open. These
        // win over the plain `d` toggle below because of the
        // modifier check.
        KeyCode::Char('u') if ctrl => Some(Action::DetailScrollUp),
        KeyCode::Char('d') if ctrl => Some(Action::DetailScrollDown),
        KeyCode::Up | KeyCode::Char('k') => Some(Action::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::Down),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),
        KeyCode::Home | KeyCode::Char('g') => Some(Action::Home),
        KeyCode::End | KeyCode::Char('G') => Some(Action::End),
        KeyCode::Enter | KeyCode::Char('o') => Some(Action::OpenInBrowser),
        KeyCode::Char('y') => Some(Action::YankUrl),
        KeyCode::Tab => Some(Action::NextTab),
        KeyCode::BackTab => Some(Action::PrevTab),
        // `d` (no modifiers) toggles the right-half detail panel.
        KeyCode::Char('d') => Some(Action::ToggleDetails),
        // `a` approve/unapprove — only meaningful with the detail
        // panel open (otherwise approve_pr would fire on a stale
        // approval state). The app method gates on details_visible.
        KeyCode::Char('a') => Some(Action::ToggleApproval),
        KeyCode::Char(c @ '1'..='9') => Some(Action::SwitchTab((c as u8 - b'1') as usize)),
        _ => None,
    }
}

pub async fn apply(action: Action, app: &mut App) -> bool {
    // Track focused key so we can lazy-fetch a new detail when the
    // user arrow-keys to a different row with the panel open.
    let pre_key = app.focused_key();
    match action {
        Action::Quit => return true,
        Action::Refresh => {
            if app.details_visible {
                app.invalidate_focused_detail();
            }
            app.refresh_active().await;
            if app.details_visible {
                app.ensure_focused_detail().await;
            }
        }
        Action::Up => app.move_selection(-1),
        Action::Down => app.move_selection(1),
        Action::PageUp => app.move_selection(-10),
        Action::PageDown => app.move_selection(10),
        Action::Home => app.move_selection(-(i32::MAX as isize)),
        Action::End => app.move_selection(i32::MAX as isize),
        Action::OpenInBrowser => app.open_focused(),
        Action::YankUrl => app.yank_focused_url(),
        Action::NextTab => {
            let next = (app.active_tab + 1) % app.tabs.len();
            app.switch_tab(next);
            if app.tabs[app.active_tab].last_fetched.is_none() {
                app.refresh_active().await;
            }
        }
        Action::PrevTab => {
            let prev = if app.active_tab == 0 {
                app.tabs.len() - 1
            } else {
                app.active_tab - 1
            };
            app.switch_tab(prev);
            if app.tabs[app.active_tab].last_fetched.is_none() {
                app.refresh_active().await;
            }
        }
        Action::SwitchTab(i) => {
            app.switch_tab(i);
            if app.tabs[app.active_tab].last_fetched.is_none() {
                app.refresh_active().await;
            }
        }
        Action::ToggleDetails => app.toggle_details().await,
        Action::DetailScrollUp => app.scroll_detail(-4),
        Action::DetailScrollDown => app.scroll_detail(4),
        Action::ToggleApproval => {
            if app.details_visible {
                app.toggle_approval().await;
            }
        }
    }
    // After a navigation action, if the focused key changed and the
    // detail pane is open, fetch the new PR's detail. Reset the pane
    // scroll so a new PR starts at the top.
    if app.details_visible {
        let post_key = app.focused_key();
        if post_key != pre_key {
            app.details_scroll = 0;
            app.ensure_focused_detail().await;
        }
    }
    false
}
