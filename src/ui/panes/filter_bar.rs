use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::state::{AppState, StatusFilter};
use crate::tmux::PaneStatus;

use crate::ui::text::display_width;

/// Render the status filter bar.
pub(crate) fn render_filter_bar<'a>(
    state: &AppState,
    width: u16,
) -> (Line<'a>, Vec<(StatusFilter, usize, usize)>) {
    let theme = &state.theme;
    let icons = &state.icons;
    let (all, running, background, waiting, idle, error) = state.status_counts();

    let icon_for = |s: PaneStatus| (icons.status_icon(&s), theme.status_color(&s, false));
    let items: Vec<(StatusFilter, (&str, ratatui::style::Color), usize)> = vec![
        (StatusFilter::All, (icons.all_icon(), theme.status_all), all),
        (
            StatusFilter::Running,
            icon_for(PaneStatus::Running),
            running,
        ),
        (
            StatusFilter::Background,
            icon_for(PaneStatus::Background),
            background,
        ),
        (
            StatusFilter::Waiting,
            icon_for(PaneStatus::Waiting),
            waiting,
        ),
        (StatusFilter::Idle, icon_for(PaneStatus::Idle), idle),
        (StatusFilter::Error, icon_for(PaneStatus::Error), error),
    ];

    let mut spans: Vec<Span<'a>> = Vec::new();
    spans.push(Span::raw(" "));
    let content_width: usize = items
        .iter()
        .map(|(_, (icon, _), count)| display_width(icon) + 1 + count.to_string().len())
        .sum();
    let spacing = (width as usize).saturating_sub(content_width + 2);
    let gap = spacing / 5;
    let extra = spacing % 5;
    let mut column = 1;
    let mut targets = Vec::new();

    for (i, (filter, (icon, icon_color), count)) in items.into_iter().enumerate() {
        if i > 0 {
            let padding = gap + usize::from(i <= extra);
            spans.push(Span::raw(" ".repeat(padding)));
            column += padding;
        }

        let is_selected = state.global.status_filter == filter;
        let icon_style = if is_selected {
            Style::default().fg(icon_color)
        } else {
            Style::default().fg(theme.filter_inactive)
        };
        spans.push(Span::styled(icon.to_string(), icon_style));
        spans.push(Span::raw(" "));

        let count_str = format!("{count}");
        let item_width = display_width(icon) + 1 + count_str.len();
        targets.push((filter, column, column + item_width));
        column += item_width;
        let count_style = if count == 0 {
            Style::default().fg(theme.filter_inactive)
        } else {
            Style::default().fg(theme.text_active)
        };
        spans.push(Span::styled(count_str, count_style));
    }

    (Line::from(spans), targets)
}

#[cfg(test)]
use crate::group::PaneGitInfo;

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    // ─── render_filter_bar tests ──────────────────────────────

    fn make_state_with_groups(groups: Vec<crate::group::RepoGroup>) -> AppState {
        let mut state = AppState::new("%99".into());
        state.repo_groups = groups;
        state.rebuild_row_targets();
        state
    }

    fn filter_bar_text(state: &AppState) -> String {
        let (line, _) = render_filter_bar(state, 41);
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn render_filter_bar_is_status_only() {
        let state = make_state_with_groups(vec![]);
        let text = filter_bar_text(&state);
        assert!(
            !text.contains("▾"),
            "status filter bar should not contain repo button"
        );
    }

    #[test]
    fn render_filter_bar_uses_selected_and_inactive_icon_colors() {
        let pane1 = crate::tmux::PaneInfo {
            pane_id: "%2".into(),
            pane_active: true,
            status: PaneStatus::Running,
            attention: false,
            agent: crate::tmux::AgentType::Claude,
            path: String::new(),
            current_command: String::new(),
            prompt: String::new(),
            prompt_is_response: false,
            started_at: None,
            wait_reason: String::new(),
            permission_mode: crate::tmux::PermissionMode::Default,
            subagents: vec![],
            pane_pid: None,
            worktree: crate::tmux::WorktreeMetadata::default(),
            session_id: None,
            session_name: String::new(),
            tmux_session_name: String::new(),
            model: String::new(),
            sidebar_spawned: false,
            bg_shell_cmd: None,
        };
        let pane2 = crate::tmux::PaneInfo {
            pane_id: "%3".into(),
            pane_active: false,
            status: PaneStatus::Idle,
            attention: false,
            agent: crate::tmux::AgentType::Codex,
            path: String::new(),
            current_command: String::new(),
            prompt: String::new(),
            prompt_is_response: false,
            started_at: None,
            wait_reason: String::new(),
            permission_mode: crate::tmux::PermissionMode::Default,
            subagents: vec![],
            pane_pid: None,
            worktree: crate::tmux::WorktreeMetadata::default(),
            session_id: None,
            session_name: String::new(),
            tmux_session_name: String::new(),
            model: String::new(),
            sidebar_spawned: false,
            bg_shell_cmd: None,
        };
        let mut state = make_state_with_groups(vec![crate::group::RepoGroup {
            name: "project".into(),
            has_focus: true,
            panes: vec![
                (pane1, PaneGitInfo::default()),
                (pane2, PaneGitInfo::default()),
            ],
        }]);
        state.global.status_filter = StatusFilter::Running;
        let theme = &state.theme;

        let (line, _) = render_filter_bar(&state, 41);
        let cells: Vec<_> = line
            .spans
            .iter()
            .filter(|span| !span.content.as_ref().trim().is_empty())
            .collect();

        assert_eq!(cells.len(), 12);

        assert_eq!(cells[0].content.as_ref(), "≡");
        assert_eq!(cells[0].style.fg, Some(theme.filter_inactive));
        assert!(!cells[0].style.add_modifier.contains(Modifier::UNDERLINED));

        assert_eq!(cells[1].content.as_ref(), "2");
        assert_eq!(cells[1].style.fg, Some(theme.text_active));

        assert_eq!(cells[2].content.as_ref(), "●");
        assert_eq!(cells[2].style.fg, Some(theme.status_running));
        assert!(!cells[2].style.add_modifier.contains(Modifier::UNDERLINED));

        assert_eq!(cells[3].content.as_ref(), "1");
        assert_eq!(cells[3].style.fg, Some(theme.text_active));

        assert_eq!(cells[4].content.as_ref(), "◎");
        assert_eq!(cells[4].style.fg, Some(theme.filter_inactive));

        assert_eq!(cells[5].content.as_ref(), "0");
        assert_eq!(cells[5].style.fg, Some(theme.filter_inactive));

        assert_eq!(cells[6].content.as_ref(), "◐");
        assert_eq!(cells[6].style.fg, Some(theme.filter_inactive));

        assert_eq!(cells[7].content.as_ref(), "0");
        assert_eq!(cells[7].style.fg, Some(theme.filter_inactive));

        assert_eq!(cells[8].content.as_ref(), "○");
        assert_eq!(cells[8].style.fg, Some(theme.filter_inactive));

        assert_eq!(cells[9].content.as_ref(), "1");
        assert_eq!(cells[9].style.fg, Some(theme.text_active));

        assert_eq!(cells[10].content.as_ref(), "✕");
        assert_eq!(cells[10].style.fg, Some(theme.filter_inactive));

        assert_eq!(cells[11].content.as_ref(), "0");
        assert_eq!(cells[11].style.fg, Some(theme.filter_inactive));
    }
}
