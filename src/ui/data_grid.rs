use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Cell, Paragraph, Row, Table, TableState},
};
use unicode_width::UnicodeWidthStr;
use uuid::Uuid;

use crate::{
    db::catalog::CatalogKind,
    db::{query::ResultSet, value::CellValue},
    security::sanitize_terminal_text,
};

use super::{
    GridHorizontalScrollTarget, GridHorizontalScrollTargets, HitRegion, HitTarget, UiState,
    icons::IconSet, theme::Theme,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VisibleColumn {
    index: usize,
    natural_width: u16,
    rendered_width: u16,
}

impl VisibleColumn {
    fn is_complete(self) -> bool {
        self.rendered_width == self.natural_width
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    tab_id: Uuid,
    result: &ResultSet,
    grid: crate::model::tab::DataGridState,
    overrides: &[Option<u16>],
    theme: Theme,
    block: Block<'_>,
    state: &mut UiState,
    edit: Option<&crate::model::relation_edit::RelationEditSession>,
    visual_range: Option<(usize, usize)>,
    icons: IconSet,
    sort_projection: Option<&[Option<crate::sql::RelationColumnSort>]>,
    sort_interactive: bool,
) {
    if result.columns.is_empty() {
        frame.render_widget(
            ratatui::widgets::Paragraph::new(format!(
                "Statement complete · {} row(s) affected",
                result.affected_rows
            ))
            .block(block)
            .style(Style::new().fg(theme.accent).bg(theme.surface))
            .alignment(ratatui::layout::Alignment::Center),
            area,
        );
        return;
    }

    let table_area = block.inner(area);
    let row_count = edit.map_or(result.rows.len(), |session| session.rows.len());
    let widths = automatic_widths(result, edit, icons, sort_projection)
        .into_iter()
        .enumerate()
        .map(|(index, width)| {
            overrides
                .get(index)
                .and_then(|value| *value)
                .unwrap_or(width)
        })
        .collect::<Vec<_>>();
    let number_width = row_number_width(row_count);
    let fixed_width = number_width.saturating_add(1);
    let available = table_area
        .width
        .saturating_sub(2)
        .saturating_sub(fixed_width)
        .max(1);
    let overflow = total_width(&widths) > available;
    let first = viewport_start(&widths, grid.column_offset, grid.selected_column, available);
    let visible = visible_columns(&widths, first, available);
    state.grid_horizontal_scroll = Some(GridHorizontalScrollTargets {
        left: horizontal_scroll_target(&widths, first.saturating_sub(1), available),
        right: horizontal_scroll_target(
            &widths,
            first
                .saturating_add(1)
                .min(last_page_start(&widths, available)),
            available,
        ),
    });
    let constraints = grid_constraints(&visible, number_width);

    let visible_rows = table_area.height.saturating_sub(1 + u16::from(overflow)) as usize;
    let row_offset =
        row_viewport_start(row_count, visible_rows, grid.row_offset, grid.selected_row);
    state.grid_viewport = Some(crate::model::tab::DataGridViewport {
        tab_id,
        column_offset: first,
        row_offset,
        visible_rows,
    });
    let row_y = table_area.y.saturating_add(1);
    if sort_projection.is_some() && sort_interactive {
        let mut header_x = data_start_x(table_area, number_width);
        for column in &visible {
            if header_x >= table_area.right() {
                break;
            }
            state.hit_regions.push(HitRegion {
                area: Rect::new(
                    header_x,
                    table_area.y,
                    column
                        .rendered_width
                        .min(table_area.right().saturating_sub(header_x)),
                    1,
                ),
                target: HitTarget::GridColumnSort(column.index),
            });
            header_x = header_x
                .saturating_add(column.rendered_width)
                .saturating_add(1);
        }
    }
    for screen_row in 0..visible_rows.min(row_count.saturating_sub(row_offset)) {
        let row_index = row_offset.saturating_add(screen_row);
        let mut x = data_start_x(table_area, number_width);
        for column in &visible {
            let width = column.rendered_width;
            if x >= table_area.right() {
                break;
            }
            state.hit_regions.push(HitRegion {
                area: Rect::new(
                    x,
                    row_y.saturating_add(screen_row as u16),
                    width.min(table_area.right().saturating_sub(x)),
                    1,
                ),
                target: HitTarget::ResultCell {
                    row: row_index,
                    column: column.index,
                },
            });
            x = x.saturating_add(width).saturating_add(1);
        }
    }
    let mut boundary_x = data_start_x(table_area, number_width);
    for column in &visible {
        boundary_x = boundary_x.saturating_add(column.rendered_width);
        if column.is_complete() && boundary_x < table_area.right().saturating_sub(1) {
            let target = HitTarget::RelationColumnResize {
                column: column.index,
                width: column.natural_width,
            };
            state.hit_regions.push(HitRegion {
                area: Rect::new(boundary_x, table_area.y, 1, 1),
                target: target.clone(),
            });
            state.hit_regions.push(HitRegion {
                area: Rect::new(boundary_x, row_y, 1, 1),
                target,
            });
        }
        boundary_x = boundary_x.saturating_add(1);
    }

    let header = Row::new(header_cells(
        &visible,
        result,
        number_width,
        theme,
        icons,
        sort_projection,
    ));
    let rows = (0..visible_rows.min(row_count.saturating_sub(row_offset))).map(|screen_row| {
        let row_index = row_offset.saturating_add(screen_row);
        let row = edit
            .map(|session| session.rows[row_index].current.as_slice())
            .unwrap_or_else(|| result.rows[row_index].as_slice());
        let editable = edit.and_then(|session| session.rows.get(row_index));
        let row_style = editable.and_then(|row| match row.state {
            crate::model::relation_edit::EditableRowState::Deleted => Some(
                Style::new()
                    .fg(theme.muted)
                    .bg(theme.row_deleted_background)
                    .add_modifier(Modifier::DIM),
            ),
            crate::model::relation_edit::EditableRowState::Updated { .. } => None,
            crate::model::relation_edit::EditableRowState::InsertDraft
            | crate::model::relation_edit::EditableRowState::Inserted => {
                Some(Style::new().fg(theme.row_inserted))
            }
            crate::model::relation_edit::EditableRowState::Conflict { .. } => {
                Some(Style::new().fg(theme.row_deleted))
            }
            crate::model::relation_edit::EditableRowState::Clean => None,
        });
        let visual_selected =
            visual_range.is_some_and(|(start, end)| row_index >= start && row_index <= end);
        // Deleted rows keep their marker even inside a visual selection.
        let row_style = if visual_selected
            && !editable.is_some_and(|row| {
                matches!(
                    row.state,
                    crate::model::relation_edit::EditableRowState::Deleted
                )
            }) {
            Some(row_style.unwrap_or_default().bg(theme.selection))
        } else {
            row_style
        };
        let changed_columns = editable.and_then(|row| match &row.state {
            crate::model::relation_edit::EditableRowState::Updated { changed_columns } => {
                Some(changed_columns)
            }
            _ => None,
        });
        Row::new(body_cells(
            &visible,
            row_index,
            number_width,
            row,
            row_style,
            changed_columns,
            theme,
        ))
    });
    let selected_row_deleted = edit
        .and_then(|session| session.rows.get(grid.selected_row))
        .is_some_and(|row| {
            matches!(
                row.state,
                crate::model::relation_edit::EditableRowState::Deleted
            )
        });
    let row_highlight_style = if selected_row_deleted {
        Style::new()
            .fg(theme.muted)
            .bg(theme.row_deleted_background)
            .add_modifier(Modifier::DIM)
    } else {
        Style::new().bg(theme.selection)
    };
    let table = Table::new(rows, constraints)
        .header(header)
        .block(block)
        .column_spacing(0)
        .row_highlight_style(row_highlight_style)
        .cell_highlight_style(
            Style::new()
                .bg(theme.accent)
                .fg(theme.background)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌");
    let selected_cell = (row_count > 0).then(|| {
        let selected_column = visible
            .iter()
            .position(|column| column.index == grid.selected_column)
            .map_or_else(|| selected_data_cell(0), selected_data_cell);
        let selected_row = grid.selected_row.saturating_sub(row_offset);
        (selected_row, selected_column)
    });
    let mut table_state = TableState::new().with_selected_cell(selected_cell);
    frame.render_stateful_widget(table, area, &mut table_state);
    if result.rows.is_empty() && table_area.height >= 2 {
        frame.render_widget(
            Paragraph::new("No rows")
                .style(Style::new().fg(theme.muted).bg(theme.surface))
                .alignment(Alignment::Center),
            Rect::new(
                table_area.x.saturating_add(1),
                table_area.y.saturating_add(1),
                table_area.width.saturating_sub(2),
                1,
            ),
        );
    }
    if overflow {
        let complete_visible = visible
            .iter()
            .filter(|column| column.is_complete())
            .count()
            .max(1);
        render_scrollbar(
            frame,
            table_area,
            first,
            complete_visible,
            widths.len(),
            last_page_start(&widths, available),
            number_width,
            theme,
            state,
        );
    }
}

fn automatic_widths(
    result: &ResultSet,
    edit: Option<&crate::model::relation_edit::RelationEditSession>,
    icons: IconSet,
    sort_projection: Option<&[Option<crate::sql::RelationColumnSort>]>,
) -> Vec<u16> {
    let rows = edit
        .map(|session| session.rows.iter().map(|row| row.current.as_slice()))
        .into_iter()
        .flatten()
        .chain(
            edit.is_none()
                .then(|| result.rows.iter().map(Vec::as_slice))
                .into_iter()
                .flatten(),
        );
    result
        .columns
        .iter()
        .enumerate()
        .map(|(column_index, column)| {
            let header = column_header_text(column, icons);
            let sort_width = sort_indicator_slot_width(column_index, icons, sort_projection);
            let content = rows
                .clone()
                .filter_map(|row| row.get(column_index))
                .map(|value| value.preview(40).text)
                .map(|text| UnicodeWidthStr::width(text.as_str()))
                .max()
                .unwrap_or(0);
            (UnicodeWidthStr::width(header.as_str())
                .saturating_add(sort_width)
                .saturating_add(1)
                .max(content)
                + 2)
            .clamp(6, 40) as u16
        })
        .collect()
}

fn grid_constraints(visible: &[VisibleColumn], number_width: u16) -> Vec<Constraint> {
    let mut constraints = Vec::with_capacity(visible.len().saturating_mul(2).saturating_add(2));
    constraints.push(Constraint::Length(number_width));
    constraints.push(Constraint::Length(1));
    for (position, column) in visible.iter().enumerate() {
        if position > 0 {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Length(column.rendered_width));
    }
    constraints
}

fn row_viewport_start(
    row_count: usize,
    visible_rows: usize,
    offset: usize,
    selected: usize,
) -> usize {
    if row_count == 0 || visible_rows == 0 {
        return 0;
    }
    let selected = selected.min(row_count - 1);
    let mut offset = offset.min(row_count.saturating_sub(visible_rows.min(row_count)));
    if selected < offset {
        offset = selected;
    } else if selected >= offset.saturating_add(visible_rows) {
        offset = selected + 1 - visible_rows;
    }
    offset.min(row_count.saturating_sub(visible_rows.min(row_count)))
}

fn header_cells(
    visible: &[VisibleColumn],
    result: &ResultSet,
    number_width: u16,
    theme: Theme,
    icons: IconSet,
    sort_projection: Option<&[Option<crate::sql::RelationColumnSort>]>,
) -> Vec<Cell<'static>> {
    let header_style = Style::new()
        .fg(theme.grid_header_text)
        .bg(theme.grid_header)
        .add_modifier(Modifier::BOLD);
    let separator_style = Style::new().fg(theme.grid_border).bg(theme.grid_header);
    let mut cells = Vec::with_capacity(visible.len().saturating_mul(2).saturating_add(2));
    cells.push(
        Cell::from(format!("{:>width$}", "#", width = number_width as usize)).style(header_style),
    );
    cells.push(Cell::from("│").style(separator_style));
    for (position, column) in visible.iter().enumerate() {
        if position > 0 {
            cells.push(Cell::from("│").style(separator_style));
        }
        let name = header_cell_line(
            column.index,
            &result.columns[column.index],
            column.rendered_width,
            theme,
            icons,
            sort_projection,
        );
        cells.push(Cell::from(name).style(header_style));
    }
    cells
}

fn column_header_text(column: &crate::db::query::ColumnMeta, icons: IconSet) -> String {
    format!(
        "{} {}",
        icons.catalog(CatalogKind::Column),
        sanitize_terminal_text(&column.name)
    )
}

fn sort_indicator(
    sort: &crate::sql::RelationColumnSort,
    icons: IconSet,
    projection: &[Option<crate::sql::RelationColumnSort>],
) -> String {
    let icon = match sort.direction {
        crate::sql::SortDirection::Asc => icons.sort_ascending(),
        crate::sql::SortDirection::Desc => icons.sort_descending(),
    };
    let multiple = projection.iter().flatten().nth(1).is_some();
    if multiple {
        format!("{icon}{}", sort.priority.saturating_add(1))
    } else {
        icon.to_owned()
    }
}

fn sort_indicator_slot_width(
    column_index: usize,
    icons: IconSet,
    sort_projection: Option<&[Option<crate::sql::RelationColumnSort>]>,
) -> usize {
    let Some(projection) = sort_projection else {
        return 0;
    };
    let base = [
        icons.sort_default(),
        icons.sort_ascending(),
        icons.sort_descending(),
    ]
    .into_iter()
    .map(UnicodeWidthStr::width)
    .max()
    .unwrap_or(0);
    let priority_width = projection
        .get(column_index)
        .and_then(Option::as_ref)
        .filter(|_| projection.iter().flatten().nth(1).is_some())
        .map_or(0, |sort| {
            (sort.priority.saturating_add(1)).to_string().len()
        });
    base.saturating_add(priority_width)
}

fn header_cell_line(
    column_index: usize,
    column: &crate::db::query::ColumnMeta,
    width: u16,
    theme: Theme,
    icons: IconSet,
    sort_projection: Option<&[Option<crate::sql::RelationColumnSort>]>,
) -> Line<'static> {
    let sorted = sort_projection
        .and_then(|projection| projection.get(column_index))
        .and_then(Option::as_ref)
        .is_some();
    let indicator = sort_projection.map_or_else(String::new, |projection| {
        projection
            .get(column_index)
            .and_then(Option::as_ref)
            .map_or_else(
                || icons.sort_default().to_owned(),
                |sort| sort_indicator(sort, icons, projection),
            )
    });
    let slot_width = sort_indicator_slot_width(column_index, icons, sort_projection);
    let indicator = super::truncate_to_cells(&indicator, usize::from(width));
    let indicator_width = UnicodeWidthStr::width(indicator.as_str());
    let reserved = slot_width.min(usize::from(width));
    let available = usize::from(width).saturating_sub(reserved.saturating_add(1));
    let label = super::truncate_to_cells(&column_header_text(column, icons), available);
    let used = UnicodeWidthStr::width(label.as_str());
    let indicator_padding = reserved.saturating_sub(indicator_width);
    let padding = usize::from(width)
        .saturating_sub(used)
        .saturating_sub(reserved)
        .saturating_add(indicator_padding);
    let indicator_style = if sorted {
        Style::new().fg(theme.action).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.muted).remove_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::raw(format!("{label}{}", " ".repeat(padding))),
        Span::styled(indicator, indicator_style),
    ])
}

fn body_cells(
    visible: &[VisibleColumn],
    row_index: usize,
    number_width: u16,
    row: &[CellValue],
    row_style: Option<Style>,
    changed_columns: Option<&std::collections::BTreeSet<usize>>,
    theme: Theme,
) -> Vec<Cell<'static>> {
    let separator_style = Style::new().fg(theme.grid_border).bg(theme.surface);
    let row_number_style = row_number_style(row_style, changed_columns.is_some(), theme);
    let mut cells = Vec::with_capacity(visible.len().saturating_mul(2).saturating_add(2));
    cells.push(
        Cell::from(format!(
            "{:>width$}",
            row_index.saturating_add(1),
            width = number_width as usize
        ))
        .style(row_number_style),
    );
    cells.push(Cell::from("│").style(separator_style));
    for (position, column) in visible.iter().enumerate() {
        if position > 0 {
            cells.push(Cell::from("│").style(separator_style));
        }
        let value = row.get(column.index).unwrap_or(&CellValue::Null);
        let preview = value.preview(column.rendered_width.saturating_sub(2) as usize);
        let style = match value {
            CellValue::Null => Style::new().fg(theme.muted).add_modifier(Modifier::ITALIC),
            CellValue::Unsupported { .. } => Style::new().fg(theme.warning),
            _ => Style::new().fg(theme.text),
        };
        let style = data_cell_style(
            style,
            row_style,
            changed_columns.is_some_and(|columns| columns.contains(&column.index)),
            theme,
        );
        cells.push(Cell::from(sanitize_terminal_text(&preview.text)).style(style));
    }
    cells
}

fn data_cell_style(base: Style, row_style: Option<Style>, updated: bool, theme: Theme) -> Style {
    let style = row_style.unwrap_or(base);
    if updated {
        style.bg(theme.row_updated)
    } else {
        style
    }
}

fn row_number_style(row_style: Option<Style>, updated: bool, theme: Theme) -> Style {
    let style = row_style
        .map(|style| style.fg(theme.muted))
        .unwrap_or_else(|| Style::new().fg(theme.muted));
    if updated {
        style.bg(theme.row_updated)
    } else {
        style
    }
}

fn total_width(widths: &[u16]) -> u16 {
    widths
        .iter()
        .enumerate()
        .fold(0u16, |total, (index, width)| {
            total
                .saturating_add(u16::from(index > 0))
                .saturating_add(*width)
        })
}

fn row_number_width(row_count: usize) -> u16 {
    row_count
        .max(1)
        .to_string()
        .len()
        .saturating_add(2)
        .min(u16::MAX as usize) as u16
}

fn selected_data_cell(visible_position: usize) -> usize {
    2usize.saturating_add(visible_position.saturating_mul(2))
}

fn data_start_x(area: Rect, number_width: u16) -> u16 {
    area.x
        .saturating_add(1)
        .saturating_add(number_width)
        .saturating_add(1)
}

fn visible_columns(widths: &[u16], first: usize, available: u16) -> Vec<VisibleColumn> {
    if available == 0 {
        return Vec::new();
    }

    let mut visible = Vec::new();
    let mut remaining = available;
    for (index, natural_width) in widths
        .iter()
        .copied()
        .enumerate()
        .skip(first.min(widths.len()))
    {
        let spacing = u16::from(!visible.is_empty());
        if remaining <= spacing {
            break;
        }

        remaining = remaining.saturating_sub(spacing);
        let rendered_width = natural_width.min(remaining);
        visible.push(VisibleColumn {
            index,
            natural_width,
            rendered_width,
        });
        remaining = remaining.saturating_sub(rendered_width);

        if rendered_width < natural_width || remaining == 0 {
            break;
        }
    }
    visible
}

fn horizontal_scroll_target(
    widths: &[u16],
    offset: usize,
    available: u16,
) -> GridHorizontalScrollTarget {
    let visible = visible_columns(widths, offset, available);
    let first_visible = visible.first().map_or(0, |column| column.index);
    let last_visible = visible
        .iter()
        .rev()
        .find(|column| column.is_complete())
        .or_else(|| visible.first())
        .map_or(first_visible, |column| column.index);
    GridHorizontalScrollTarget {
        offset: first_visible,
        first_visible,
        last_visible,
    }
}

fn selection_is_revealed(visible: &[VisibleColumn], selected: usize) -> bool {
    visible.iter().any(|column| {
        column.index == selected
            && (column.is_complete()
                || visible.first().is_some_and(|first| first.index == selected))
    })
}

fn viewport_start(widths: &[u16], offset: usize, selected: usize, available: u16) -> usize {
    if widths.is_empty() {
        return 0;
    }
    let selected = selected.min(widths.len() - 1);
    let mut start = offset.min(last_page_start(widths, available));
    if selected < start {
        return selected;
    }
    while !selection_is_revealed(&visible_columns(widths, start, available), selected)
        && start + 1 < widths.len()
    {
        start += 1;
    }
    start
}

fn last_page_start(widths: &[u16], available: u16) -> usize {
    let mut used = 0u16;
    for index in (0..widths.len()).rev() {
        let spacing = u16::from(used > 0);
        let next = used.saturating_add(spacing).saturating_add(widths[index]);
        if used != 0 && next > available {
            return index + 1;
        }
        used = next;
    }
    0
}

#[allow(clippy::too_many_arguments)]
fn render_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    first: usize,
    visible_column_count: usize,
    column_count: usize,
    max_offset: usize,
    number_width: u16,
    theme: Theme,
    state: &mut UiState,
) {
    let track = Rect::new(
        data_start_x(area, number_width),
        area.bottom().saturating_sub(1),
        area.width
            .saturating_sub(2)
            .saturating_sub(number_width.saturating_add(1)),
        1,
    );
    if track.width < 3 || column_count == 0 {
        return;
    }
    let rail_width = track.width.saturating_sub(2);
    let thumb_width = ((rail_width as usize * visible_column_count) / column_count)
        .clamp(1, rail_width as usize) as u16;
    let travel = rail_width.saturating_sub(thumb_width);
    let thumb_offset = ((travel as usize * first)
        .checked_div(max_offset)
        .unwrap_or(0)) as u16;
    let thumb_x = track.x.saturating_add(1).saturating_add(thumb_offset);
    let before = thumb_x.saturating_sub(track.x.saturating_add(1));
    let after = rail_width
        .saturating_sub(before)
        .saturating_sub(thumb_width);
    let line = Line::from(vec![
        Span::styled("‹", Style::new().fg(theme.muted)),
        Span::styled("─".repeat(before as usize), Style::new().fg(theme.muted)),
        Span::styled(
            "━".repeat(thumb_width as usize),
            Style::new().fg(theme.accent),
        ),
        Span::styled("─".repeat(after as usize), Style::new().fg(theme.muted)),
        Span::styled("›", Style::new().fg(theme.muted)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::new().bg(theme.surface)),
        track,
    );

    let page = visible_column_count as isize;
    state.hit_regions.push(HitRegion {
        area: Rect::new(track.x, track.y, thumb_x.saturating_sub(track.x), 1),
        target: HitTarget::GridScrollbarPage {
            offset: first.saturating_sub(page as usize),
        },
    });
    state.hit_regions.push(HitRegion {
        area: Rect::new(thumb_x, track.y, thumb_width, 1),
        target: HitTarget::GridScrollbarThumb {
            track_x: track.x.saturating_add(1),
            track_width: rail_width,
            thumb_x,
            thumb_width,
            offset: first,
            max_offset,
        },
    });
    let after_x = thumb_x.saturating_add(thumb_width);
    state.hit_regions.push(HitRegion {
        area: Rect::new(after_x, track.y, track.right().saturating_sub(after_x), 1),
        target: HitTarget::GridScrollbarPage {
            offset: first.saturating_add(page as usize).min(max_offset),
        },
    });
}

#[cfg(test)]
mod tests {
    use super::{
        GridHorizontalScrollTarget, VisibleColumn, column_header_text, data_cell_style,
        horizontal_scroll_target, row_number_style, row_number_width, row_viewport_start,
        selected_data_cell, total_width, viewport_start, visible_columns,
    };
    use ratatui::style::{Modifier, Style};
    use unicode_width::UnicodeWidthStr;

    use crate::{
        db::query::{ColumnMeta, ResultSet},
        ui::{HitTarget, icons::IconMode, icons::IconSet, theme::Theme},
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Rect, widgets::Block};

    fn hit_regions(
        column_offset: usize,
        sort_projection: Option<&[Option<crate::sql::RelationColumnSort>]>,
    ) -> crate::ui::UiState {
        let result = ResultSet {
            columns: vec![
                ColumnMeta {
                    name: "first".into(),
                    type_name: "TEXT".into(),
                },
                ColumnMeta {
                    name: "second".into(),
                    type_name: "TEXT".into(),
                },
                ColumnMeta {
                    name: "third".into(),
                    type_name: "TEXT".into(),
                },
            ],
            rows: vec![vec![
                crate::db::value::CellValue::Text("a".into()),
                crate::db::value::CellValue::Text("b".into()),
                crate::db::value::CellValue::Text("c".into()),
            ]],
            affected_rows: 0,
        };
        let mut terminal = Terminal::new(TestBackend::new(24, 8)).unwrap();
        let mut state = crate::ui::UiState::new();
        terminal
            .draw(|frame| {
                super::render(
                    frame,
                    Rect::new(0, 0, 24, 8),
                    uuid::Uuid::nil(),
                    &result,
                    crate::model::tab::DataGridState {
                        column_offset,
                        selected_column: column_offset,
                        ..Default::default()
                    },
                    &[Some(6); 3],
                    Theme::deep_space(),
                    Block::default(),
                    &mut state,
                    None,
                    None,
                    IconSet::new(IconMode::Ascii),
                    sort_projection,
                    sort_projection.is_some(),
                );
            })
            .unwrap();
        state
    }

    #[test]
    fn relation_sort_hit_regions_cover_header_content_not_separators() {
        let projection = [None, None, None];
        let state = hit_regions(0, Some(&projection));

        assert!(state.hit_regions.iter().any(|region| {
            region.target == HitTarget::GridColumnSort(0)
                && region.area.y == 0
                && region.area.width > 0
        }));
        assert!(state.hit_regions.iter().any(|region| {
            region.target
                == HitTarget::RelationColumnResize {
                    column: 0,
                    width: 6,
                }
                && region.area.y == 0
        }));
        assert_eq!(
            state.target_at(11, 0),
            Some(&HitTarget::RelationColumnResize {
                column: 0,
                width: 6,
            })
        );
        assert_eq!(state.target_at(5, 0), Some(&HitTarget::GridColumnSort(0)));
    }

    #[test]
    fn relation_sort_hit_regions_follow_horizontal_scroll() {
        let projection = [None, None, None];
        let state = hit_regions(1, Some(&projection));

        assert!(
            state
                .hit_regions
                .iter()
                .any(|region| region.target == HitTarget::GridColumnSort(1))
        );
        assert!(
            !state
                .hit_regions
                .iter()
                .any(|region| region.target == HitTarget::GridColumnSort(0))
        );
    }

    #[test]
    fn non_relation_grids_do_not_register_sort_hit_regions() {
        let state = hit_regions(0, None);
        assert!(
            !state
                .hit_regions
                .iter()
                .any(|region| matches!(region.target, HitTarget::GridColumnSort(_)))
        );
    }

    #[test]
    fn column_header_includes_explorer_column_icon() {
        let column = ColumnMeta {
            name: "user_id".to_owned(),
            type_name: "integer".to_owned(),
        };

        assert_eq!(
            column_header_text(&column, IconSet::new(IconMode::Unicode)),
            "│ user_id"
        );
    }

    #[test]
    fn sort_markers_use_muted_and_action_styles() {
        let column = ColumnMeta {
            name: "name".into(),
            type_name: "text".into(),
        };
        let projection = [
            None,
            Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Asc,
                priority: 0,
            }),
        ];
        let theme = Theme::deep_space();

        let inactive = super::header_cell_line(
            0,
            &column,
            12,
            theme,
            IconSet::new(IconMode::Unicode),
            Some(&projection),
        );
        let active = super::header_cell_line(
            1,
            &column,
            12,
            theme,
            IconSet::new(IconMode::Unicode),
            Some(&projection),
        );

        let inactive_style = inactive.spans.last().expect("sort marker span").style;
        assert_eq!(inactive_style.fg, Some(theme.muted));
        assert!(!inactive_style.add_modifier.contains(Modifier::BOLD));

        let active_style = active.spans.last().expect("sort marker span").style;
        assert_eq!(active_style.fg, Some(theme.action));
        assert!(active_style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn sort_markers_keep_direction_and_priority_in_plain_theme() {
        let column = ColumnMeta {
            name: "name".into(),
            type_name: "text".into(),
        };
        let projection = [
            Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Desc,
                priority: 0,
            }),
            Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Asc,
                priority: 1,
            }),
        ];
        let line = super::header_cell_line(
            1,
            &column,
            12,
            Theme::for_color_mode(crate::cli::ColorMode::Never),
            IconSet::new(IconMode::Unicode),
            Some(&projection),
        );

        assert_eq!(line.to_string().width(), 12);
        assert!(line.to_string().ends_with("▲2"));
        let indicator_style = line.spans.last().expect("sort marker span").style;
        assert!(indicator_style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn sorted_header_keeps_indicator_at_the_right_edge() {
        let column = ColumnMeta {
            name: "name".into(),
            type_name: "text".into(),
        };
        let text = super::header_cell_line(
            0,
            &column,
            12,
            Theme::deep_space(),
            IconSet::new(IconMode::Unicode),
            Some(&[Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Desc,
                priority: 1,
            })]),
        );
        assert_eq!(UnicodeWidthStr::width(text.to_string().as_str()), 12);
        assert!(text.to_string().ends_with('▼'), "{text:?}");
    }

    #[test]
    fn sorted_header_truncates_label_without_changing_width() {
        let column = ColumnMeta {
            name: "very-long-name".into(),
            type_name: "text".into(),
        };
        let text = super::header_cell_line(
            0,
            &column,
            6,
            Theme::deep_space(),
            IconSet::new(IconMode::Ascii),
            Some(&[Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Asc,
                priority: 0,
            })]),
        );
        assert_eq!(UnicodeWidthStr::width(text.to_string().as_str()), 6);
        assert!(text.to_string().ends_with('^'), "{text:?}");
    }

    #[test]
    fn sorted_header_shows_priority_for_multiple_terms() {
        let column = ColumnMeta {
            name: "name".into(),
            type_name: "text".into(),
        };
        let projection = [
            Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Asc,
                priority: 0,
            }),
            Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Desc,
                priority: 1,
            }),
        ];
        let text = super::header_cell_line(
            1,
            &column,
            12,
            Theme::deep_space(),
            IconSet::new(IconMode::Unicode),
            Some(&projection),
        );
        assert_eq!(UnicodeWidthStr::width(text.to_string().as_str()), 12);
        assert!(text.to_string().ends_with("▼2"), "{text:?}");
    }

    #[test]
    fn sorting_header_states_reserve_the_same_unicode_indicator_width() {
        let column = ColumnMeta {
            name: "long-header".into(),
            type_name: "text".into(),
        };
        let projections = [
            [None],
            [Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Asc,
                priority: 0,
            })],
            [Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Desc,
                priority: 0,
            })],
        ];

        let headers: Vec<_> = projections
            .iter()
            .map(|projection| {
                super::header_cell_line(
                    0,
                    &column,
                    14,
                    Theme::deep_space(),
                    IconSet::new(IconMode::Unicode),
                    Some(projection),
                )
            })
            .collect();

        assert!(headers.iter().all(|header| header.width() == 14));
        assert!(
            headers
                .iter()
                .all(|header| header.to_string().starts_with("│ long"))
        );
        assert_eq!(
            super::sort_indicator_slot_width(
                0,
                IconSet::new(IconMode::Unicode),
                Some(&projections[0])
            ),
            2
        );
        assert_eq!(
            super::sort_indicator_slot_width(
                0,
                IconSet::new(IconMode::Unicode),
                Some(&projections[1])
            ),
            2
        );
        assert_eq!(
            super::sort_indicator_slot_width(
                0,
                IconSet::new(IconMode::Unicode),
                Some(&projections[2])
            ),
            2
        );
    }

    #[test]
    fn sorting_header_stays_bounded_for_narrow_widths_and_unicode_names() {
        let column = ColumnMeta {
            name: "用户名称".into(),
            type_name: "text".into(),
        };
        let projection = [Some(crate::sql::RelationColumnSort {
            direction: crate::sql::SortDirection::Desc,
            priority: 0,
        })];

        for mode in [IconMode::NerdFont, IconMode::Unicode, IconMode::Ascii] {
            for width in [0, 1, 2, 6, 12] {
                let text = super::header_cell_line(
                    0,
                    &column,
                    width,
                    Theme::deep_space(),
                    IconSet::new(mode),
                    Some(&projection),
                );
                assert_eq!(
                    text.width(),
                    usize::from(width),
                    "mode={mode:?}, width={width}"
                );
            }
        }
    }

    #[test]
    fn sorting_header_omits_indicator_without_sort_projection() {
        let column = ColumnMeta {
            name: "name".into(),
            type_name: "text".into(),
        };
        let text = super::header_cell_line(
            0,
            &column,
            12,
            Theme::deep_space(),
            IconSet::new(IconMode::Unicode),
            None,
        );

        assert_eq!(text.width(), 12);
        assert!(!text.to_string().contains('▲'));
        assert!(!text.to_string().contains('▼'));
        assert_eq!(
            super::sort_indicator_slot_width(0, IconSet::new(IconMode::Unicode), None),
            0
        );
    }

    #[test]
    fn multiple_sort_indicator_slot_accounts_for_priority_digits() {
        let projection = [
            Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Asc,
                priority: 0,
            }),
            Some(crate::sql::RelationColumnSort {
                direction: crate::sql::SortDirection::Desc,
                priority: 9,
            }),
        ];

        assert_eq!(
            super::sort_indicator_slot_width(0, IconSet::new(IconMode::Unicode), Some(&projection)),
            3
        );
        assert_eq!(
            super::sort_indicator_slot_width(1, IconSet::new(IconMode::Unicode), Some(&projection)),
            4
        );
    }

    #[test]
    fn row_number_width_tracks_absolute_result_size() {
        assert_eq!(row_number_width(0), 3);
        assert_eq!(row_number_width(9), 3);
        assert_eq!(row_number_width(10), 4);
        assert_eq!(row_number_width(500), 5);
    }

    #[test]
    fn selected_data_cells_follow_the_fixed_gutter() {
        assert_eq!(selected_data_cell(0), 2);
        assert_eq!(selected_data_cell(1), 4);
    }

    #[test]
    fn row_number_stays_muted_when_row_is_selected() {
        let theme = Theme::deep_space();
        let selected_style = Style::new().fg(theme.text).bg(theme.selection);
        let style = row_number_style(Some(selected_style), false, theme);

        assert_eq!(style.fg, Some(theme.muted));
        assert_eq!(style.bg, Some(theme.selection));
    }

    #[test]
    fn updated_row_number_uses_update_background() {
        let theme = Theme::deep_space();
        let style = row_number_style(Some(Style::new().fg(theme.text)), true, theme);

        assert_eq!(style.fg, Some(theme.muted));
        assert_eq!(style.bg, Some(theme.row_updated));
    }

    #[test]
    fn only_updated_data_cells_use_update_background() {
        let theme = Theme::deep_space();
        let base = Style::new().fg(theme.text);

        let unchanged = data_cell_style(base, None, false, theme);
        let updated = data_cell_style(base, None, true, theme);

        assert_eq!(unchanged.fg, Some(theme.text));
        assert_eq!(unchanged.bg, None);
        assert_eq!(updated.fg, Some(theme.text));
        assert_eq!(updated.bg, Some(theme.row_updated));
    }

    #[test]
    fn narrow_grid_starts_with_first_columns() {
        let widths = vec![6; 10];
        let start = viewport_start(&widths, 0, 0, 20);
        assert_eq!(start, 0);
        assert_eq!(
            visible_columns(&widths, start, 20)
                .iter()
                .map(|column| column.index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn viewport_follows_selection_across_right_edge() {
        let widths = vec![6; 10];
        assert_eq!(viewport_start(&widths, 0, 3, 20), 1);
    }

    #[test]
    fn viewport_follows_selection_across_left_edge() {
        let widths = vec![6; 10];
        assert_eq!(viewport_start(&widths, 4, 2, 20), 2);
    }

    #[test]
    fn viewport_stays_put_at_left_edge_until_selection_crosses_it() {
        let widths = vec![6; 10];
        assert_eq!(viewport_start(&widths, 4, 4, 20), 4);
        assert_eq!(viewport_start(&widths, 4, 3, 20), 3);
    }

    #[test]
    fn explicit_last_offset_reaches_last_column() {
        let widths = vec![6; 10];
        let start = viewport_start(&widths, 9, 9, 20);
        assert_eq!(
            visible_columns(&widths, start, 20)
                .iter()
                .map(|column| column.index)
                .collect::<Vec<_>>(),
            vec![7, 8, 9]
        );
    }

    #[test]
    fn exact_fit_does_not_add_trailing_spacing() {
        let widths = vec![6, 6];
        assert_eq!(total_width(&widths), 13);
        assert_eq!(
            visible_columns(&widths, 0, 13),
            vec![
                VisibleColumn {
                    index: 0,
                    natural_width: 6,
                    rendered_width: 6,
                },
                VisibleColumn {
                    index: 1,
                    natural_width: 6,
                    rendered_width: 6,
                },
            ]
        );
    }

    #[test]
    fn remaining_space_renders_one_trailing_partial_column() {
        let widths = vec![6, 6];

        assert_eq!(
            visible_columns(&widths, 0, 10),
            vec![
                VisibleColumn {
                    index: 0,
                    natural_width: 6,
                    rendered_width: 6,
                },
                VisibleColumn {
                    index: 1,
                    natural_width: 6,
                    rendered_width: 3,
                },
            ]
        );
    }

    #[test]
    fn horizontal_scroll_target_excludes_trailing_partial_columns() {
        assert_eq!(
            horizontal_scroll_target(&[6, 20, 8, 30, 6], 1, 30),
            GridHorizontalScrollTarget {
                offset: 1,
                first_visible: 1,
                last_visible: 2,
            }
        );
        assert_eq!(
            horizontal_scroll_target(&[6, 40, 8], 1, 20),
            GridHorizontalScrollTarget {
                offset: 1,
                first_visible: 1,
                last_visible: 1,
            }
        );
    }

    #[test]
    fn visible_columns_handle_narrow_boundaries() {
        assert!(visible_columns(&[6, 6], 0, 0).is_empty());
        assert_eq!(
            visible_columns(&[6, 6], 0, 7),
            vec![VisibleColumn {
                index: 0,
                natural_width: 6,
                rendered_width: 6,
            }]
        );
        assert_eq!(
            visible_columns(&[20, 6], 0, 10),
            vec![VisibleColumn {
                index: 0,
                natural_width: 20,
                rendered_width: 10,
            }]
        );
        assert!(visible_columns(&[6, 6], 2, 20).is_empty());
    }

    #[test]
    fn trailing_partial_column_stops_the_visible_range() {
        assert_eq!(
            visible_columns(&[6, 6, 6], 0, 10),
            vec![
                VisibleColumn {
                    index: 0,
                    natural_width: 6,
                    rendered_width: 6,
                },
                VisibleColumn {
                    index: 1,
                    natural_width: 6,
                    rendered_width: 3,
                },
            ]
        );
    }

    #[test]
    fn viewport_scrolls_when_selection_enters_a_trailing_partial_column() {
        assert_eq!(viewport_start(&[6, 6, 6], 0, 1, 10), 1);
    }

    #[test]
    fn oversized_selected_column_is_valid_as_the_first_column() {
        let widths = vec![6, 20, 6];
        let start = viewport_start(&widths, 0, 1, 10);

        assert_eq!(start, 1);
        assert_eq!(visible_columns(&widths, start, 10)[0].rendered_width, 10);
    }

    #[test]
    fn row_viewport_scrolls_only_after_selection_crosses_an_edge() {
        assert_eq!(row_viewport_start(10, 3, 0, 2), 0);
        assert_eq!(row_viewport_start(10, 3, 0, 3), 1);
        assert_eq!(row_viewport_start(10, 3, 3, 3), 3);
        assert_eq!(row_viewport_start(10, 3, 3, 2), 2);
    }
}
