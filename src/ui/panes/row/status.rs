use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use super::ctx::RowCtx;
use crate::tmux::PaneStatus;
use crate::ui::icons::StatusIcons;
use crate::ui::text::{display_width, elapsed_label, truncate_to_width};

pub(super) fn status_row(
    pane: &crate::tmux::PaneInfo,
    ctx: &RowCtx,
    icons: &StatusIcons,
    spinner_frame: usize,
    now: u64,
) -> Line<'static> {
    let theme = ctx.theme;
    let (icon, pulse_color) = running_icon_for(&pane.status, spinner_frame, icons);
    let icon_color =
        pulse_color.unwrap_or_else(|| theme.status_color(&pane.status, pane.attention));
    let model = compact_model(&pane.model);
    let name = if pane.session_name.is_empty() {
        pane.agent.label()
    } else {
        &pane.session_name
    };
    let fixed_width = display_width(icon) + 2;
    let elapsed = elapsed_label(pane.started_at, now);
    let elapsed_reserved = display_width(&elapsed) + usize::from(!elapsed.is_empty());
    let available = ctx
        .inner_width
        .saturating_sub(fixed_width + elapsed_reserved);
    let model_budget =
        display_width(&model).min(available.saturating_sub(display_width(name).min(6)));
    let model = truncate_to_width(&model, model_budget);
    let name = truncate_to_width(name, available.saturating_sub(display_width(&model)));
    let left_width = fixed_width + display_width(&model) + display_width(&name);
    let elapsed = if display_width(&elapsed) < ctx.inner_width.saturating_sub(left_width) {
        elapsed
    } else {
        String::new()
    };
    let elapsed_width = display_width(&elapsed);
    let left_spans = vec![
        Span::styled(
            icon.to_string(),
            ctx.apply_bg(Style::default().fg(icon_color)),
        ),
        Span::styled(
            format!(" {model}"),
            ctx.apply_bg(Style::default().fg(theme.model_name)),
        ),
        Span::styled(
            format!(" {name}"),
            ctx.apply_bg(Style::default().fg(theme.agent_name)),
        ),
    ];
    let right_spans = vec![Span::styled(
        elapsed,
        ctx.apply_bg(Style::default().fg(theme.text_muted)),
    )];
    ctx.row_line_split(left_spans, left_width, right_spans, elapsed_width)
}

fn compact_model(model: &str) -> String {
    if model.is_empty() {
        return "model pending".into();
    }
    for family in ["opus", "sonnet", "haiku"] {
        if let Some(version) = model.strip_prefix(&format!("claude-{family}-")) {
            let version = version
                .split('-')
                .take_while(|part| part.len() < 8)
                .collect::<Vec<_>>()
                .join(".");
            return format!(
                "{}{} {version}",
                family[..1].to_ascii_uppercase(),
                &family[1..]
            );
        }
    }
    model.to_string()
}
pub(super) fn running_icon_for<'a>(
    status: &PaneStatus,
    spinner_frame: usize,
    icons: &'a StatusIcons,
) -> (&'a str, Option<Color>) {
    use crate::SPINNER_PULSE;

    match status {
        PaneStatus::Running => {
            let color_idx = SPINNER_PULSE[spinner_frame % SPINNER_PULSE.len()];
            (icons.status_icon(status), Some(Color::Indexed(color_idx)))
        }
        _ => (icons.status_icon(status), None),
    }
}
