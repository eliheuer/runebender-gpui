// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The glyph grid's geometry and order.
//!
//! Cell sizes, the visible window of rows, the glyph order the grid
//! shows, and multi-selection across cells.

use super::*;

impl Workspace {
    /// Solve the grid's cell size against the measured viewport, the
    /// way the web editor does: the zoom slider sets a *target* size,
    /// then columns are chosen to fill the width exactly and the row
    /// height divides the visible height evenly, so no row is left
    /// sliced in half at the bottom edge.
    pub(crate) fn grid_cell_metrics(&self) -> GridFit {
        // Detail mode needs room for the info lines: the cell floor
        // rises, whatever the zoom slider says.
        let size = if self.font_view_mode == FontViewMode::Detail {
            self.grid_cell_size.max(148.0)
        } else {
            self.grid_cell_size
        };
        Self::solve_grid(self.grid_viewport, size, GRID_PAD)
    }

    /// Same solve for the editor sidebar's mini grid, against its own
    /// narrower viewport.
    pub(crate) fn sidebar_cell_metrics(&self) -> GridFit {
        Self::solve_grid(self.sidebar_viewport, self.sidebar_cell_size, GRID_PAD_SM)
    }

    /// Scroll a row-quantized grid by a wheel delta. The offset is
    /// kept in whole rows, so a row is never left sliced at the top or
    /// bottom edge — the web got this from `scroll-snap-type`, which
    /// gpui has no equivalent for.
    pub(crate) fn scroll_grid_rows(
        offset: &mut usize,
        delta_y: f32,
        row_h: f32,
        rows_visible: usize,
        rows_total: usize,
    ) -> bool {
        let max = rows_total.saturating_sub(rows_visible);
        let step = (delta_y / row_h.max(1.0)).abs().ceil() as usize;
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

    pub(crate) fn solve_grid(viewport: gpui::Size<gpui::Pixels>, target: f32, pad: f32) -> GridFit {
        let label_h = |w: f32| cell_label_metrics(w).height;
        let target = target.max(24.0);
        let vw: f32 = viewport.width.into();
        let vh: f32 = viewport.height.into();
        if vw <= 0.0 || vh <= 0.0 {
            // First frame, before the probe reports: fall back to the
            // target size.
            return GridFit {
                cell_w: target,
                cell_h: target + label_h(target),
                cols: 1,
                rows: 1,
            };
        }
        let usable_w = (vw - pad * 2.0).max(target);
        let cols = (((usable_w + GRID_GAP) / (target + GRID_GAP)).floor() as usize).max(1);
        let cell_w = ((usable_w - GRID_GAP * (cols - 1) as f32) / cols as f32).floor();

        let target_row = cell_w + label_h(cell_w);
        let usable_h = (vh - pad.min(GRID_PAD_Y) * 2.0).max(target_row);
        let rows = (((usable_h + GRID_GAP) / (target_row + GRID_GAP)).round() as usize).max(1);
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
                self.sidebar_matches
                    .as_ref()
                    .is_none_or(|m| m.contains(entry.name.as_ref()))
                    && self.search_matches(entry.name.as_ref(), entry.codepoint)
            })
            .collect();
        if !self.sort_unicode {
            indices.sort_by_key(|&i| font.glyphs[i].name.clone());
        }
        indices
    }

    /// Cmd-click: toggle a glyph in the multi-selection.
    pub(crate) fn grid_toggle_multi(&mut self, index: usize) {
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(primary) = self.selected
            && let Some(primary_name) = self.font().map(|f| f.glyphs[primary].name.to_string())
        {
            self.multi_selected.insert(primary_name);
        }
        if !self.multi_selected.remove(&name) {
            self.multi_selected.insert(name);
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
        self.multi_selected.extend(names);
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
                self.multi_selected.contains(entry.name.as_ref())
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
            query: self.search_query.clone(),
            mode: self.search_mode,
            regex: self.search_regex,
            case: self.search_case,
            sort_unicode: self.sort_unicode,
            filter: self.sidebar_filter.clone(),
            revision: self.font().map(|f| f.revision).unwrap_or(0),
            master: self.project.as_ref().map(|p| p.active).unwrap_or(0),
        };
        if self.order_key.as_ref() == Some(&key)
            && let Some(order) = &self.glyph_order
        {
            return order.clone();
        }
        let matches = self.sidebar_matches.clone();
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
                if !self.sort_unicode {
                    // Font order is already unicode order, so the Name
                    // toggle sorts alphabetically.
                    indices.sort_by(|a, b| font.glyphs[*a].name.cmp(&font.glyphs[*b].name));
                }
                indices
            }
            None => Vec::new(),
        };
        let order = Arc::new(order);
        self.glyph_order = Some(order.clone());
        self.order_key = Some(key);
        order
    }

    /// The cached order, for the panels that only hold `&self`.
    /// `render` refreshes it once a frame before any of them run.
    pub(crate) fn glyph_order(&self) -> Arc<Vec<usize>> {
        self.glyph_order.clone().unwrap_or_default()
    }
}
