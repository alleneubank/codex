//! Layout helpers for the custom status line row owned by the chat composer footer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::bottom_pane::footer::render_footer_line;

pub(super) fn custom_status_line_height(status_line: Option<&Line<'static>>, padding: u16) -> u16 {
    status_line.map(|_| padding.saturating_add(1)).unwrap_or(0)
}

pub(super) fn render_custom_status_line(
    area: Rect,
    buf: &mut Buffer,
    status_line: Line<'static>,
    padding: u16,
) {
    let padding = padding.min(area.height.saturating_sub(1));
    let line_area = Rect {
        y: area.y.saturating_add(padding),
        height: area.height.min(1),
        ..area
    };
    render_footer_line(line_area, buf, status_line);
}
