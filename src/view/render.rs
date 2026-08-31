// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The render tree: how the workspace's state becomes a frame.

use crate::AddExtremes;
use crate::BakeMasks;
use crate::Balance;
use crate::BoldenWithModel;
use crate::BooleanExclude;
use crate::BooleanIntersect;
use crate::BooleanSubtract;
use crate::BooleanUnion;
use crate::CheckJoining;
use crate::CopyContours;
use crate::CopySelectedGlyphs;
use crate::CorrectPathDirection;
use crate::CubicsToQuads;
use crate::Decompose;
use crate::DeselectAllPoints;
use crate::DuplicateGlyph;
use crate::DuplicateRepeat;
use crate::DuplicateSelection;
use crate::ExportFont;
use crate::ExportGlyphSvg;
use crate::FilterExtrude;
use crate::FilterOffsetCurve;
use crate::FilterRoughen;
use crate::FilterSlant;
use crate::FlipHorizontal;
use crate::FlipVertical;
use crate::Harmonize;
use crate::HyperToCubic;
use crate::ImportSvg;
use crate::InvertPointSelection;
use crate::MeasureAllOff;
use crate::MeasureAllOn;
use crate::MeasureColorize;
use crate::MeasureHandles;
use crate::MeasurePopcount;
use crate::MeasureSegments;
use crate::MeasureSideBearings;
use crate::MeasureSizes;
use crate::MeasureSpans;
use crate::Mode;
use crate::NewFont;
use crate::NewGlyph;
use crate::NextMaster;
use crate::NextSampleString;
use crate::OpenFont;
use crate::Optimize;
use crate::PasteContours;
use crate::PlaceImage;
use crate::PreviousMaster;
use crate::PreviousSampleString;
use crate::QuadsToCubics;
use crate::Redo;
use crate::Reinterpolate;
use crate::RemoveGlyphCmd;
use crate::RemoveImage;
use crate::RemoveOverlap;
use crate::ReverseContours;
use crate::Rotate180;
use crate::RotateLeft;
use crate::RotateRight;
use crate::RoundCoordinates;
use crate::RoundCorners;
use crate::SaveFont;
use crate::SaveFontAs;
use crate::SelectAllPoints;
use crate::SetStartPoint;
use crate::SetThemeDark;
use crate::SetThemeGray;
use crate::SetThemeLight;
use crate::SetThemeMidnight;
use crate::ShowAllMasters;
use crate::SortByName;
use crate::SortByUnicode;
use crate::SyncMetrics;
use crate::TidyPaths;
use crate::TraceImage;
use crate::Undo;
use crate::Workspace;
use crate::ZoomToFit;
use crate::launch::ui_font_family;
use crate::view::grid::glyph_column_span;
use crate::view::grid::pack_spans;
use crate::view::theme as t;
use crate::widgets;
use crate::workspace::FontViewMode;
use crate::workspace::GRID_GAP;
use crate::workspace::Tool;
use gpui::Bounds;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::Window;
use gpui::canvas;
use gpui::div;
use gpui::prelude::FluentBuilder;
use gpui::px;
use kurbo::Affine;
use runebender_core::outline::glyph_ops::CurveOp;

/// The label a sidebar tab shows on hover, now that the tabs are
/// icons. Placeholder icons for the two that have none of their own.
/// The hover tooltip on a sidebar tab icon.
pub(crate) struct TabTooltip {
    /// The tab's full name.
    pub(crate) label: &'static str,
}

impl Render for TabTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_1p5()
            .py_0p5()
            .bg(t::panel_bg())
            .border(t::stroke())
            .border_color(t::panel_outline())
            .rounded(t::radius())
            .text_xs()
            .text_color(t::text())
            .child(self.label)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Claim focus only when nothing else has it, so text inputs
        // (the search box) keep theirs while focused.
        if window.focused(cx).is_none() {
            window.focus(&self.focus_handle, cx);
        }

        self.ensure_axis_sliders(window, cx);
        self.ensure_cell_slider(window, cx);
        self.ensure_sidebar_slider(window, cx);
        self.ensure_preview_slider(window, cx);
        self.ensure_model_strength_slider(window, cx);
        if self.sidebar.counts.is_none() && self.project.is_some() {
            self.rebuild_sidebar_cache();
        }
        // One filter-and-sort pass per frame at most, and none at all
        // when nothing that decides the order has changed.
        self.visible_glyphs();
        self.refresh_metric_inputs(false, window, cx);
        self.refresh_font_info_inputs(false, window, cx);
        self.refresh_features_input(false, window, cx);
        if matches!(self.mode, Mode::Editor(_)) {
            self.refresh_coord_inputs(false, window, cx);
        }
        self.refresh_glyph_inputs(false, window, cx);
        use widgets::resizable::{h_resizable, resizable_panel, v_resizable};

        // Glyphs-style docked layout: left sidebar | center | right
        // sidebar as flat resizable panels, no floating containers.
        let (left, center): (gpui::AnyElement, gpui::AnyElement) = match self.mode {
            Mode::Editor(index) if self.project.is_some() => (
                self.editor_sidebar(cx).into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_h(px(0.0))
                    // Canvas over preview, with a draggable divider:
                    // the preview takes whatever height it is given and
                    // fits its type to it. The bottom bar belongs to
                    // this column too, so the side panels keep the
                    // window's full height and the dividers between
                    // the three columns run all the way down.
                    .child(
                        div().flex_1().min_h(px(0.0)).child(
                            v_resizable("editor-column")
                                .child(
                                    resizable_panel().child(
                                        // A flex column: the canvas
                                        // grows into the panel. In a
                                        // plain block its flex_1 is
                                        // ignored and it lays out at
                                        // zero height.
                                        div()
                                            .size_full()
                                            .min_h(px(0.0))
                                            .flex()
                                            .flex_col()
                                            .child(self.editor_view(index, cx)),
                                    ),
                                )
                                .child(
                                    resizable_panel()
                                        .size(px(140.0))
                                        .size_range(px(0.0)..px(720.0))
                                        .visible(self.preview.visible)
                                        .child(self.preview_strip(cx)),
                                ),
                        ),
                    )
                    .child(self.status_bar(cx))
                    .into_any_element(),
            ),
            _ => {
                let _query = self.sidebar.search_query.clone();
                let fit = self.grid_cell_metrics();
                let (cell_w, cell_h) = (fit.cell_w, fit.cell_h);
                let mut rows_total = 0usize;
                let indices = self.glyph_order();
                let mut visible_rows: Vec<Vec<(usize, usize)>> = Vec::new();
                let grid: Vec<_> = match self.font() {
                    Some(font) => {
                        // A wide advance or a long name takes more
                        // than one column, and the last cell on a row
                        // grows into whatever is left, so every row
                        // fills the width and no name is cut off.
                        let upm = font.units_per_em;
                        let spans: Vec<(usize, usize)> = indices
                            .iter()
                            .map(|&i| {
                                (
                                    i,
                                    glyph_column_span(
                                        font.glyphs[i].name.as_ref(),
                                        font.glyphs[i].advance,
                                        upm,
                                    ),
                                )
                            })
                            .collect();
                        let packed = pack_spans(&spans, fit.cols);
                        rows_total = packed.len();
                        // Only the rows on screen are built: the view
                        // starts at a row boundary and holds exactly
                        // the rows that fit, so nothing is ever half
                        // drawn at either edge.
                        let start = self.grid.scroll_row.min(rows_total.saturating_sub(1));
                        visible_rows = packed.iter().skip(start).take(fit.rows).cloned().collect();
                        packed
                            .into_iter()
                            .skip(start)
                            .take(fit.rows)
                            .flatten()
                            .map(|(i, span)| {
                                let w = cell_w * span as f32 + GRID_GAP * (span - 1) as f32;
                                self.glyph_cell_sized(i, w, cell_h, false, cx)
                                    .into_any_element()
                            })
                            .collect()
                    }
                    None => Vec::new(),
                };
                // The grid solves its own layout against the viewport,
                // so it needs the viewport measured. An inert canvas
                // laid over the scroll area reports its bounds back.
                let this = cx.entity().downgrade();
                let probe = canvas(
                    move |bounds: Bounds<gpui::Pixels>, _, app: &mut gpui::App| {
                        this.update(app, |this, cx| {
                            if this.grid.viewport != bounds.size {
                                this.grid.viewport = bounds.size;
                                cx.notify();
                            }
                        })
                        .ok();
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full();
                (
                    self.category_sidebar(cx).into_any_element(),
                    div()
                        .size_full()
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .child({
                            let grid_block = div()
                                .id("glyph-grid")
                                .flex_1()
                                .min_h(px(0.0))
                                .relative()
                                .overflow_hidden()
                                .child(probe)
                                .child(
                                    // The block is sized to exactly
                                    // cols x rows and centred, so the
                                    // pixels left over by rounding the
                                    // cell size split evenly between
                                    // the margins instead of piling up
                                    // on the right and bottom.
                                    div()
                                        .size_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .w(px(fit.content_w()))
                                                .flex()
                                                .flex_wrap()
                                                .gap(px(GRID_GAP))
                                                .children(grid),
                                        ),
                                )
                                .children(self.glyph_overlay(visible_rows, fit, cx))
                                .on_scroll_wheel(cx.listener(
                                    move |this, ev: &gpui::ScrollWheelEvent, _, cx| {
                                        let dy = match ev.delta {
                                            gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                                            gpui::ScrollDelta::Lines(p) => p.y * 24.0,
                                        };
                                        if Self::scroll_grid_rows(
                                            &mut this.grid.scroll_row,
                                            dy,
                                            fit.cell_h + GRID_GAP,
                                            fit.rows,
                                            rows_total,
                                        ) {
                                            cx.notify();
                                        }
                                    },
                                ));
                            // List swaps the whole grid for the
                            // property table; Grid and Detail share
                            // the cell pipeline.
                            match self.grid.view_mode {
                                FontViewMode::List => self.glyph_list_view(cx),
                                FontViewMode::Matrix => self.glyph_matrix_view(cx),
                                _ => grid_block.into_any_element(),
                            }
                        })
                        // One bar per column bottom: the grid's lives
                        // here so the sidebars run past it.
                        .child(self.status_bar(cx))
                        .into_any_element(),
                )
            }
        };
        let in_editor = matches!(self.mode, Mode::Editor(_)) && self.project.is_some();
        let right = div()
            .id("right-sidebar")
            .size_full()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .when(in_editor, |el| {
                el.child(self.glyph_info_panel(cx))
                    .child(self.selection_section(cx))
                    .child(self.transform_section(cx))
                    .child(self.curves_section(cx))
                    .child(self.background_section(cx))
                    .child(self.color_section(cx))
                    .child(self.shaping_section(cx))
                    .child(self.related_section(cx))
                    .child(self.layers_section(cx))
                    .children(self.axes_section(cx))
            })
            .when(!in_editor, |el| {
                el.child(self.glyph_info_panel(cx))
                    .child(self.font_info_section(cx))
                    .child(self.dimensions_section(cx))
                    .child(self.kerning_section(cx))
                    .child(self.groups_section(cx))
                    .child(self.compare_section(cx))
                    .child(self.features_section(cx))
                    .child(self.layers_section(cx))
                    .child(self.glyph_preview_panel())
            });
        let content = div()
            .flex_1()
            .min_h(px(0.0))
            .child(
                h_resizable("workspace")
                    .child(
                        resizable_panel()
                            .size(px(224.0))
                            .size_range(px(140.0)..px(440.0))
                            .visible(!self.left_collapsed)
                            .child(
                                // No border here: the resize handle
                                // already paints a 1px divider in the
                                // same color, and the two together
                                // read as a thick line.
                                div().size_full().bg(t::panel_bg()).child(left),
                            ),
                    )
                    .child(resizable_panel().child(center))
                    .child(
                        resizable_panel()
                            .size(px(230.0))
                            .size_range(px(170.0)..px(440.0))
                            .child(div().size_full().bg(t::panel_bg()).child(right)),
                    ),
            )
            .into_any_element();

        // The window's text style. This used to come from the
        // wrapper the app was mounted in; without it gpui has no
        // family to shape with and no UI text draws at all.
        window.set_rem_size(px(16.0));

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t::window_bg())
            .font_family(ui_font_family(cx))
            .text_color(t::text())
            .text_size(px(13.0))
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &OpenFont, _, cx| {
                this.open_dialog(cx);
            }))
            .on_action(cx.listener(|this, _: &NewFont, _, cx| {
                this.command_new_font();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SaveFontAs, _, cx| {
                this.command_save_as(cx);
            }))
            .on_action(cx.listener(|this, _: &SaveFont, _, cx| {
                this.command_save(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ExportFont, _, cx| {
                this.command_export(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.undo();
                this.rebuild_text_models();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.redo();
                this.rebuild_text_models();
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &CopyContours, window: &mut Window, cx| {
                    // A focused field handles its own clipboard.
                    if widgets::input::any_field_focused(window, cx) {
                        return;
                    }
                    this.command_copy();
                    cx.notify();
                }),
            )
            .on_action(
                cx.listener(|this, _: &PasteContours, window: &mut Window, cx| {
                    if widgets::input::any_field_focused(window, cx) {
                        return;
                    }
                    this.command_paste_routed(cx);
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &CopySelectedGlyphs, _, cx| {
                this.command_copy_selection_text(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &MeasureColorize, _, cx| {
                this.toggle_measure(|o| o.colorize = !o.colorize, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureHandles, _, cx| {
                this.toggle_measure(|o| o.handles = !o.handles, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureSegments, _, cx| {
                this.toggle_measure(|o| o.segments = !o.segments, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureSizes, _, cx| {
                this.toggle_measure(|o| o.sizes = !o.sizes, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureSpans, _, cx| {
                this.toggle_measure(|o| o.spans = !o.spans, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureSideBearings, _, cx| {
                this.toggle_measure(|o| o.sidebearings = !o.sidebearings, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasurePopcount, _, cx| {
                this.toggle_measure(|o| o.popcount = !o.popcount, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureAllOn, _, cx| {
                this.toggle_measure(
                    |o| {
                        o.colorize = true;
                        o.handles = true;
                        o.segments = true;
                        o.spans = true;
                        o.sidebearings = true;
                        o.sizes = true;
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MeasureAllOff, _, cx| {
                this.toggle_measure(
                    |o| {
                        o.colorize = false;
                        o.handles = false;
                        o.segments = false;
                        o.spans = false;
                        o.sidebearings = false;
                        o.sizes = false;
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SetThemeDark, window, cx| {
                this.command_set_theme("dark", window, cx);
            }))
            .on_action(cx.listener(|this, _: &SetThemeMidnight, window, cx| {
                this.command_set_theme("midnight", window, cx);
            }))
            .on_action(cx.listener(|this, _: &SetThemeGray, window, cx| {
                this.command_set_theme("gray", window, cx);
            }))
            .on_action(cx.listener(|this, _: &SetThemeLight, window, cx| {
                this.command_set_theme("light", window, cx);
            }))
            .on_action(cx.listener(|this, _: &RemoveOverlap, _, cx| {
                this.command_remove_overlap();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Decompose, _, cx| {
                this.command_decompose();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FlipHorizontal, _, cx| {
                this.apply_transform(Affine::scale_non_uniform(-1.0, 1.0));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FlipVertical, _, cx| {
                this.apply_transform(Affine::scale_non_uniform(1.0, -1.0));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RotateLeft, _, cx| {
                this.apply_transform(Affine::rotate(std::f64::consts::FRAC_PI_2));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RotateRight, _, cx| {
                this.apply_transform(Affine::rotate(-std::f64::consts::FRAC_PI_2));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ReverseContours, _, cx| {
                this.command_reverse();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BooleanUnion, _, cx| {
                this.command_boolean(linesweeper::BinaryOp::Union);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BooleanSubtract, _, cx| {
                this.command_boolean(linesweeper::BinaryOp::Difference);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BooleanIntersect, _, cx| {
                this.command_boolean(linesweeper::BinaryOp::Intersection);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BooleanExclude, _, cx| {
                this.command_boolean(linesweeper::BinaryOp::Xor);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DuplicateSelection, _, cx| {
                this.command_duplicate();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DuplicateRepeat, _, cx| {
                this.command_duplicate_repeat();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &HyperToCubic, _, cx| {
                if let Mode::Editor(index) = this.mode {
                    this.push_undo_snapshot(index);
                    let selected = this.editor.selected.clone();
                    let ok = this
                        .font_mut()
                        .and_then(|f| {
                            f.edit_glyph(index, |g| {
                                runebender_core::outline::glyph_ops::convert_hyper_to_cubic(
                                    g, &selected,
                                )
                            })
                        })
                        .unwrap_or(false);
                    if !ok {
                        this.editor.undo.pop();
                    } else {
                        this.editor.selected.clear();
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &AddExtremes, _, cx| {
                this.command_add_extremes();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TidyPaths, _, cx| {
                this.command_tidy_paths();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CorrectPathDirection, _, cx| {
                this.command_correct_path_direction();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RoundCoordinates, _, cx| {
                this.command_round_coordinates();
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &SelectAllPoints, window: &mut Window, cx| {
                    if widgets::input::any_field_focused(window, cx) {
                        return;
                    }
                    this.command_select_points(0);
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &DeselectAllPoints, _, cx| {
                this.command_select_points(1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &InvertPointSelection, _, cx| {
                this.command_select_points(2);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NewGlyph, _, cx| {
                this.command_add_glyph();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DuplicateGlyph, _, cx| {
                this.command_duplicate_glyph();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RemoveGlyphCmd, _, cx| {
                this.command_remove_glyph();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FilterOffsetCurve, _, cx| {
                if let Ok(delta) = this.inputs.offset.read(cx).value().trim().parse::<f64>() {
                    this.command_offset(delta);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FilterExtrude, _, cx| {
                let text = this.inputs.extrude.read(cx).value().to_string();
                this.command_extrude(&text);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FilterRoughen, _, cx| {
                let text = this.inputs.roughen.read(cx).value().to_string();
                this.command_roughen(&text);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FilterSlant, _, cx| {
                if let Ok(deg) = this.inputs.slant.read(cx).value().trim().parse::<f64>()
                    && deg != 0.0
                    && deg.abs() < 89.0
                {
                    // Positive leans right, the italic convention.
                    this.apply_transform(Affine::skew(deg.to_radians().tan(), 0.0));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ExportGlyphSvg, _, cx| {
                this.command_export_glyph_svg();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Reinterpolate, _, cx| {
                this.command_reinterpolate();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SyncMetrics, _, cx| {
                this.command_sync_metrics();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BakeMasks, _, cx| {
                this.command_bake_masks();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CheckJoining, _, cx| {
                this.command_check_joining();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &QuadsToCubics, _, cx| {
                this.command_convert_curves(true);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CubicsToQuads, _, cx| {
                this.command_convert_curves(false);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowAllMasters, _, cx| {
                this.show_all_masters = !this.show_all_masters;
                this.status_note = Some(
                    if this.show_all_masters {
                        "All masters shown · click any master's node to edit it"
                    } else {
                        "Showing the active master"
                    }
                    .into(),
                );
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NextSampleString, _, cx| {
                this.command_sample_string(1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PreviousSampleString, _, cx| {
                this.command_sample_string(-1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RoundCorners, _, cx| {
                this.command_round_corners();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PlaceImage, _, cx| {
                this.command_place_image(cx);
            }))
            .on_action(cx.listener(|this, _: &ImportSvg, _, cx| {
                this.command_import_svg(cx);
            }))
            .on_action(cx.listener(|this, _: &RemoveImage, _, cx| {
                this.command_remove_image();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BoldenWithModel, _, cx| {
                this.command_bolden_with_model(cx);
            }))
            .on_action(cx.listener(|this, _: &TraceImage, _, cx| {
                this.command_trace_image(cx);
            }))
            .on_action(cx.listener(|this, _: &Rotate180, _, cx| {
                this.apply_transform(Affine::rotate(std::f64::consts::PI));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SetStartPoint, _, cx| {
                this.command_set_start_point();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Harmonize, _, cx| {
                this.apply_curve_op(CurveOp::Harmonize);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Balance, _, cx| {
                this.apply_curve_op(CurveOp::Balance);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Optimize, _, cx| {
                this.apply_curve_op(CurveOp::Optimize(0.12));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SortByName, _, cx| {
                this.grid.sort_unicode = false;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SortByUnicode, _, cx| {
                this.grid.sort_unicode = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| {
                if matches!(this.mode, Mode::Editor(_)) {
                    this.editor.initialized = false;
                    this.ensure_editor_fit();
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &NextMaster, _, cx| {
                this.command_step_master(1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PreviousMaster, _, cx| {
                this.command_step_master(-1);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if this.handle_key(event, window, cx) {
                    cx.notify();
                }
            }))
            .on_key_up(cx.listener(|this, event: &gpui::KeyUpEvent, _, cx| {
                if matches!(
                    event.keystroke.key.as_str(),
                    "left" | "right" | "up" | "down"
                ) {
                    this.nudging = false;
                }
                if event.keystroke.key.as_str() == "space" && this.editor.tool == Tool::Preview {
                    this.editor.tool = this.editor.previous_tool;
                    cx.notify();
                }
            }))
            .child(self.header(cx))
            .child(content)
    }
}
