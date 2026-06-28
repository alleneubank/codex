//! Layout helpers for the custom status line row owned by the chat composer footer.

use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::bottom_pane::footer::render_footer_line;

pub(super) fn custom_status_line_height(status_line: Option<&Line<'static>>, padding: u16) -> u16 {
    status_line.map(|_| padding.saturating_add(1)).unwrap_or(0)
}

pub(super) fn render_custom_status_line(
    area: Rect,
    buf: &mut Buffer,
    status_line: Option<Line<'static>>,
    padding: u16,
) -> Rect {
    let custom_status_line_height = custom_status_line_height(status_line.as_ref(), padding);
    let [custom_status_rect, hint_container_rect] = if custom_status_line_height > 0 {
        Layout::vertical([
            Constraint::Length(custom_status_line_height),
            Constraint::Min(0),
        ])
        .areas(area)
    } else {
        [Rect::default(), area]
    };

    if let Some(line) = status_line {
        let [_, line_rect] = Layout::vertical([Constraint::Length(padding), Constraint::Length(1)])
            .areas(custom_status_rect);
        render_footer_line(line_rect, buf, line);
    }

    hint_container_rect
}
