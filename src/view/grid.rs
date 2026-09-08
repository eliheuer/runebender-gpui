// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The glyph grid's geometry and order.
//!
//! Cell sizes, the visible window of rows, the glyph order the grid
//! shows, and multi-selection across cells.

use crate::Arc;
use crate::Workspace;
use crate::view::render::to_count;
use crate::workspace::GRID_GAP;
use crate::workspace::GRID_PAD;
use crate::workspace::GRID_PAD_SM;
use crate::workspace::GRID_PAD_Y;
use crate::workspace::GridFit;
use crate::workspace::SidebarFilter;
use crate::workspace::UI_TEXT_PX;
use kurbo::Affine;
impl Workspace {
    /// Solve the grid's cell size against the measured viewport.
    ///
    /// The zoom slider sets a *target* size. Columns are then chosen
    /// to fill the width exactly, and the row height divides the
    /// visible height evenly, so no row is left sliced in half at the
    /// bottom edge. This is how the web editor sizes its grid.
    pub(crate) fn grid_cell_metrics(&self) -> GridFit {
        Self::solve_grid(self.grid.viewport, self.grid.cell_size, GRID_PAD)
    }

    /// Same solve for the editor sidebar's mini grid, against its own
    /// narrower viewport. Its rows are slightly taller than they are
    /// wide, which gives the thumbnail outline a calmer vertical centre
    /// without making the column count jump.
    pub(crate) fn sidebar_cell_metrics(&self) -> GridFit {
        Self::solve_grid_with_row_factor(
            self.sidebar.viewport,
            self.sidebar.cell_size,
            GRID_PAD_SM,
            1.18,
        )
    }

    /// Scroll a row-quantized grid by a wheel delta.
    ///
    /// The offset is kept in whole rows, so a row is never left
    /// sliced at the top or bottom edge. The web editor got this from
    /// `scroll-snap-type`, which gpui has no equivalent for.
    pub(crate) fn scroll_grid_rows(
        offset: &mut usize,
        delta_y: f32,
        row_h: f32,
        rows_visible: usize,
        rows_total: usize,
    ) -> bool {
        let max = rows_total.saturating_sub(rows_visible);
        let step = (delta_y / row_h.max(1.0)).abs().ceil().max(1.0);
        let step = usize::try_from(to_count(step)).unwrap_or(1);
        let step = step.clamp(1, rows_visible.max(1));
        let next = if delta_y > 0.0 {
            offset.saturating_sub(step)
        } else {
            (*offset + step).min(max)
        };
        let changed = next != *offset;
        *offset = next;
        changed
    }

    /// The shared solve behind both grids: columns from the `target`
    /// size, then width and height divided evenly. Falls back to one
    /// target-size cell before the viewport reports a size.
    pub(crate) fn solve_grid(viewport: gpui::Size<gpui::Pixels>, target: f32, pad: f32) -> GridFit {
        Self::solve_grid_with_row_factor(viewport, target, pad, 1.0)
    }

    /// The shared grid solve with a caller-selected target row aspect.
    fn solve_grid_with_row_factor(
        viewport: gpui::Size<gpui::Pixels>,
        target: f32,
        pad: f32,
        row_factor: f32,
    ) -> GridFit {
        let label_h = |w: f32| cell_label_metrics(w).height;
        let target = target.max(24.0);
        let vw: f32 = viewport.width.into();
        let vh: f32 = viewport.height.into();
        if vw <= 0.0 || vh <= 0.0 {
            // First frame, before the probe reports: populate enough
            // cells that opening a glyph never looks like an empty
            // sidebar with one stranded square. The measured viewport
            // replaces this compact, sidebar-sized estimate on the next
            // frame.
            let fallback_w = 280.0;
            let fallback_h = 560.0;
            let usable_w = (fallback_w - pad * 2.0).max(target);
            let cols = usize::try_from(to_count((usable_w + GRID_GAP) / (target + GRID_GAP)))
                .unwrap_or(1)
                .max(1);
            let cell_w = ((usable_w - GRID_GAP * (cols - 1) as f32) / cols as f32).floor();
            let target_row = (cell_w + label_h(cell_w)) * row_factor;
            let usable_h = (fallback_h - pad.min(GRID_PAD_Y) * 2.0).max(target_row);
            let rows = usize::try_from(to_count((usable_h + GRID_GAP) / (target_row + GRID_GAP)))
                .unwrap_or(1)
                .max(1);
            let cell_h = ((usable_h - GRID_GAP * (rows - 1) as f32) / rows as f32).floor();
            return GridFit {
                cell_w,
                cell_h,
                cols,
                rows,
            };
        }
        let usable_w = (vw - pad * 2.0).max(target);
        let cols = usize::try_from(to_count((usable_w + GRID_GAP) / (target + GRID_GAP)))
            .unwrap_or(1)
            .max(1);
        let cell_w = ((usable_w - GRID_GAP * (cols - 1) as f32) / cols as f32).floor();

        let target_row = (cell_w + label_h(cell_w)) * row_factor;
        let usable_h = (vh - pad.min(GRID_PAD_Y) * 2.0).max(target_row);
        let rows = usize::try_from(to_count((usable_h + GRID_GAP) / (target_row + GRID_GAP)))
            .unwrap_or(1)
            .max(1);
        let cell_h = ((usable_h - GRID_GAP * (rows - 1) as f32) / rows as f32).floor();
        GridFit {
            cell_w,
            cell_h,
            cols,
            rows,
        }
    }

    /// The grid's visible order (same filter + sort the grid draws).
    pub(crate) fn visible_grid_indices(&self) -> Vec<usize> {
        let Some(font) = self.font() else {
            return Vec::new();
        };
        let mut indices: Vec<usize> = (0..font.glyphs.len())
            .filter(|&i| {
                let entry = &font.glyphs[i];
                self.sidebar
                    .matches
                    .as_ref()
                    .is_none_or(|m| m.contains(entry.name.as_ref()))
                    && self.search_matches(entry.name.as_ref(), entry.codepoint)
            })
            .collect();
        if !self.grid.sort_unicode {
            indices.sort_by_key(|&i| font.glyphs[i].name.clone());
        }
        indices
    }

    /// Move the primary grid selection by a cell offset in the visible
    /// order and scroll it into view. Arrow navigation is intentionally
    /// bounded at the grid edges rather than wrapping to another row.
    pub(crate) fn step_grid_selection(&mut self, offset: isize) -> bool {
        let order = self.visible_grid_indices();
        if order.is_empty() || offset == 0 {
            return false;
        }
        let current = self
            .selected
            .and_then(|selected| order.iter().position(|&index| index == selected))
            .unwrap_or(0);
        let target = current
            .saturating_add_signed(offset)
            .min(order.len().saturating_sub(1));
        if target == current && self.selected.is_some() {
            return false;
        }
        self.selected = Some(order[target]);
        self.grid.multi_selected.clear();

        let fit = self.grid_cell_metrics();
        let row = target / fit.cols.max(1);
        if row < self.grid.scroll_row {
            self.grid.scroll_row = row;
        } else if row >= self.grid.scroll_row + fit.rows.max(1) {
            self.grid.scroll_row = row + 1 - fit.rows.max(1);
        }
        true
    }

    /// Cmd-click: toggle a glyph in the multi-selection.
    pub(crate) fn grid_toggle_multi(&mut self, index: usize) {
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(primary) = self.selected
            && let Some(primary_name) = self.font().map(|f| f.glyphs[primary].name.to_string())
        {
            self.grid.multi_selected.insert(primary_name);
        }
        if !self.grid.multi_selected.remove(&name) {
            self.grid.multi_selected.insert(name);
        }
        self.selected = Some(index);
    }

    /// Shift-click: extend from the primary through the visible order.
    pub(crate) fn grid_extend_multi(&mut self, index: usize) {
        let order = self.visible_grid_indices();
        let Some(primary) = self.selected else {
            self.selected = Some(index);
            return;
        };
        let (Some(a), Some(b)) = (
            order.iter().position(|&i| i == primary),
            order.iter().position(|&i| i == index),
        ) else {
            self.selected = Some(index);
            return;
        };
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let names: Vec<String> = self
            .font()
            .map(|font| {
                order[lo..=hi]
                    .iter()
                    .map(|&i| font.glyphs[i].name.to_string())
                    .collect()
            })
            .unwrap_or_default();
        self.grid.multi_selected.extend(names);
    }

    /// Every selected glyph name (primary plus multi), in font order.
    pub(crate) fn selection_names(&self) -> Vec<String> {
        let Some(font) = self.font() else {
            return Vec::new();
        };
        let mut names: Vec<String> = font
            .glyphs
            .iter()
            .filter(|entry| {
                self.grid.multi_selected.contains(entry.name.as_ref())
                    || self
                        .selected
                        .is_some_and(|i| font.glyphs[i].name == entry.name)
            })
            .map(|entry| entry.name.to_string())
            .collect();
        names.dedup();
        names
    }

    /// The glyphs to show, filtered and sorted, from cache when the
    /// inputs have not moved.
    pub(crate) fn visible_glyphs(&mut self) -> Arc<Vec<usize>> {
        let key = OrderKey {
            query: self.sidebar.search_query.clone(),
            mode: self.sidebar.search_mode,
            regex: self.sidebar.search_regex,
            case: self.sidebar.search_case,
            sort_unicode: self.grid.sort_unicode,
            filter: self.sidebar.filter.clone(),
            revision: self.font().map(|f| f.revision).unwrap_or(0),
            master: self.project.as_ref().map(|p| p.active).unwrap_or(0),
        };
        if self.grid.order_key.as_ref() == Some(&key)
            && let Some(order) = &self.grid.order
        {
            return order.clone();
        }
        let matches = self.sidebar.matches.clone();
        let order: Vec<usize> = match self.font() {
            Some(font) => {
                let mut indices: Vec<usize> = (0..font.glyphs.len())
                    .filter(|&i| {
                        let entry = &font.glyphs[i];
                        matches
                            .as_ref()
                            .is_none_or(|m| m.contains(entry.name.as_ref()))
                            && self.search_matches(entry.name.as_ref(), entry.codepoint)
                    })
                    .collect();
                if !self.grid.sort_unicode {
                    // Font order is already unicode order, so the Name
                    // toggle sorts alphabetically.
                    indices.sort_by(|a, b| font.glyphs[*a].name.cmp(&font.glyphs[*b].name));
                }
                indices
            }
            None => Vec::new(),
        };
        let order = Arc::new(order);
        self.grid.order = Some(order.clone());
        self.grid.order_key = Some(key);
        order
    }

    /// The cached order, for the panels that only hold `&self`.
    /// `render` refreshes it once a frame before any of them run.
    pub(crate) fn glyph_order(&self) -> Arc<Vec<usize>> {
        self.grid.order.clone().unwrap_or_default()
    }
}

// ---- cell placement, shared by the grid and the sidebar's mini grid ----

/// Where a glyph's outline sits inside a cell, as an affine from
/// design space to the cell's local pixels.
///
/// One em-based scale keeps a period small and an M tall, while the
/// visible ink itself is centred in the available thumbnail box. The
/// em window grows rather than cropping ink that runs past it. This is
/// a port of the web editor's grid thumbnail box in `glyph_svg.rs`.
pub(crate) fn cell_glyph_transform(
    ink: kurbo::Rect,
    empty: bool,
    advance: f64,
    upm: f64,
    w: f64,
    h: f64,
) -> Affine {
    const EM_FILL: f64 = 0.65;
    const BASELINE_FROM_TOP: f64 = 0.8;
    // A small inset prevents a centred outline from looking crowded
    // against a thin cell rule.
    const THUMBNAIL_FILL: f64 = 0.92;
    let (ink_x0, ink_w) = if empty || ink.width() <= 0.0 {
        (0.0, advance.max(1.0))
    } else {
        (ink.x0, ink.width())
    };
    let em_height = upm.max(1.0) / EM_FILL;
    let em_top = -BASELINE_FROM_TOP * em_height;
    let (top, bottom) = if empty {
        (em_top, em_top + em_height)
    } else {
        (em_top.min(-ink.y1), (em_top + em_height).max(-ink.y0))
    };
    let box_h = (bottom - top).max(1.0);
    let scale = (w / ink_w).min(h / box_h) * THUMBNAIL_FILL;
    let x_offset = (w - ink_w * scale) / 2.0 - ink_x0 * scale;
    let y_offset = if empty {
        // Empty glyphs have no visible ink to centre; keep the em's
        // baseline placement for their advance-width placeholder.
        (h - box_h * scale) / 2.0 - top * scale
    } else {
        // GPUI's y axis points down while font y points up. Put ink.y1
        // at the top inset, so the actual outline (not its em box) has
        // the same air above and below. This is particularly important
        // for flat letters such as E, whose baseline box is asymmetric.
        (h - ink.height() * scale) / 2.0 + ink.y1 * scale
    };
    Affine::translate((x_offset, y_offset)) * Affine::scale_non_uniform(scale, -scale)
}

/// A cell's label block: whether it shows at all, its type size, and
/// the height it takes.
///
/// Uses one compact inset on every side, with close leading so names
/// and Unicode sit together in the cell's lower-left corner.
pub(crate) fn cell_label_metrics(cell_w: f32) -> CellLabels {
    // gpui's default line box is much taller than the type size, which
    // clipped the first line and pushed the two apart. The line height
    // is stated here and the block's height is derived from it, so the
    // box always holds exactly what it draws.
    const PAD: f32 = 5.0;
    const GAP: f32 = 0.0;
    let build = |size: f32, lines: usize| {
        let line = (size * 1.10).ceil();
        CellLabels {
            show: true,
            size,
            line,
            height: PAD + line * lines as f32 + GAP * (lines.saturating_sub(1)) as f32 + PAD,
        }
    };
    // One type size everywhere, the interface's own. A cell too
    // narrow to carry a name at that size carries none.
    if cell_w < 48.0 {
        CellLabels {
            show: false,
            size: 0.0,
            line: 0.0,
            height: 0.0,
        }
    } else if cell_w < 90.0 {
        // Name only.
        build(UI_TEXT_PX, 1)
    } else {
        build(UI_TEXT_PX, 2)
    }
}

/// Everything that decides which glyphs show and in what order. When
/// this is unchanged, the order is too.
#[derive(Clone, PartialEq)]
pub(crate) struct OrderKey {
    /// The search box text.
    pub(crate) query: String,
    /// The search scope: 0 is all, 1 is name, 2 is unicode.
    pub(crate) mode: u8,
    /// Whether the query is treated as a regex.
    pub(crate) regex: bool,
    /// Whether the search is case-sensitive.
    pub(crate) case: bool,
    /// True for unicode order, false for name order.
    pub(crate) sort_unicode: bool,
    /// The sidebar's category filter.
    pub(crate) filter: SidebarFilter,
    /// Structural changes to the font (a glyph added, removed or
    /// renamed) bump this.
    pub(crate) revision: u64,
    /// Masters can differ in what they contain.
    pub(crate) master: usize,
}

/// The label block's type size, line height and total height.
#[derive(Clone, Copy)]
pub(crate) struct CellLabels {
    /// Whether the cell shows labels at all.
    pub(crate) show: bool,
    /// The label type size, in pixels.
    pub(crate) size: f32,
    /// The stated line height, in pixels.
    pub(crate) line: f32,
    /// The whole block's height, padding included, in pixels.
    pub(crate) height: f32,
}

/// How many columns a glyph should take.
///
/// A long name or a wide advance gets more room instead of being cut
/// off. This is a port of the web editor's `computeGlyphColumnSpan`.
pub(crate) fn glyph_column_span(name: &str, advance: f64, upm: f64) -> usize {
    let name_span = match name.chars().count() {
        0..=14 => 1,
        15..=26 => 2,
        _ => 3,
    };
    let ratio = if upm > 0.0 { advance / upm } else { 0.0 };
    let width_span = if ratio <= 1.5 {
        1
    } else if ratio <= 2.8 {
        2
    } else if ratio <= 4.0 {
        3
    } else {
        4
    };
    name_span.max(width_span)
}

/// Pack spanned cells into rows that each fill the width exactly.
///
/// When the next cell will not fit, the last one on the row grows
/// into the gap. This is the web editor's `gridGlyphItems`. Returns
/// one vector per row of (item index, span).
pub(crate) fn pack_spans(spans: &[(usize, usize)], cols: usize) -> Vec<Vec<(usize, usize)>> {
    let cols = cols.max(1);
    let mut rows: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut row: Vec<(usize, usize)> = Vec::new();
    let mut used = 0_usize;
    for &(item, span) in spans {
        let span = span.clamp(1, cols);
        if used + span > cols && !row.is_empty() {
            if let Some(last) = row.last_mut() {
                last.1 += cols - used;
            }
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        row.push((item, span));
        used += span;
        if used == cols {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
    }
    if !row.is_empty() {
        if let Some(last) = row.last_mut() {
            last.1 += cols - used;
        }
        rows.push(row);
    }
    rows
}
