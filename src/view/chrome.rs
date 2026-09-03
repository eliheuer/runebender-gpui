// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The bars around the canvas: header, status bar, preview, sliders.
//!
//! Each `ensure_*` creates a control once and keeps it in step with
//! the workspace on later renders.

use crate::Mode;
use crate::Workspace;
use crate::view::paint::IconMark;
use crate::view::paint::eye_icon;
use crate::view::paint::flat_slider;
use crate::view::paint::glyph_free_icon;
use crate::view::paint::icon_svg;
use crate::view::paint::invert_icon;
use crate::view::render::px32;
use crate::view::theme as t;
use crate::widgets;
use crate::workspace::BAR_BUTTON;
use crate::workspace::BOTTOM_BAR_H;
use crate::workspace::CELL;
use crate::workspace::Drag;
use crate::workspace::FontViewMode;
use crate::workspace::MINI_CELL;
use crate::workspace::TAB_H;
use gpui::AppContext;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::Window;
use gpui::div;
use gpui::prelude::FluentBuilder;
use gpui::px;
impl Workspace {
    /// The header bar: the file name and its save status.
    pub(crate) fn header(&self, cx: &mut Context<'_, Self>) -> impl IntoElement + use<> {
        let (title, status): (SharedString, SharedString) = match (self.font(), &self.load_error) {
            (Some(font), _) => (
                // Just the file name, like Glyphs' title. The glyph
                // count lives in the status bar; upm belongs to font
                // info, not the chrome.
                font.source_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| font.source_path.display().to_string())
                    .into(),
                if font.dirty {
                    "Not saved".into()
                } else {
                    match &self.last_save_label {
                        Some(at) => format!("Saved {at}").into(),
                        None => "Saved".into(),
                    }
                },
            ),
            (None, Some(err)) => ("Load failed".into(), err.clone()),
            (None, None) => ("Runebender".into(), "No font loaded".into()),
        };
        let in_editor = matches!(self.mode, Mode::Editor(_));
        div()
            .flex()
            .items_center()
            // The same 6px everywhere: from the window's edges to the
            // icon, and from the icon to the title.
            .gap_1p5()
            .px_1p5()
            .py_1p5()
            .bg(t::panel_bg())
            .border_b_1()
            .border_color(t::panel_outline())
            .child(
                div()
                    .id("toggle-left")
                    .w(px(TAB_H))
                    .h(px(TAB_H))
                    .rounded(t::radius_control())
                    .cursor_pointer()
                    .child(icon_svg(
                        "glyph-grid",
                        if self.left_collapsed {
                            t::text_muted()
                        } else {
                            t::text()
                        },
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.left_collapsed = !this.left_collapsed;
                        cx.notify();
                    })),
            )
            .when(cfg!(not(target_os = "macos")), |el| {
                #[cfg(not(target_os = "macos"))]
                let el = el.child(div().flex_none().child(self.app_menu_bar.clone()));
                el
            })
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .overflow_hidden()
                    .child(div().text_color(t::text()).child(title))
                    // "Saved" is the quiet state; only "Not saved" earns
                    // the warning colour.
                    .child(
                        div()
                            .text_color(if self.font().is_some_and(|f| f.dirty) {
                                t::status_yellow()
                            } else {
                                t::text_muted()
                            })
                            .child(status),
                    ),
            )
            .when(
                // Always up in the editor, the Glyphs bottom-corner
                // toggle: direction is a property of the review, not
                // of the text tool.
                in_editor,
                |el| el.child(self.direction_toolbar(cx)),
            )
            .when(in_editor, |el| el.child(self.header_tools(cx)))
            .child(self.tab_strip(cx))
    }

    /// Create the axis sliders once a project with axes exists.
    pub(crate) fn ensure_axis_sliders(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if !self.axis_sliders.is_empty() || project.axes.is_empty() || project.model.is_none() {
            return;
        }
        let axes = project.axes.clone();
        for (i, axis) in axes.iter().enumerate() {
            if axis.max <= axis.min {
                continue; // degenerate axis: nothing to slide
            }
            // Start where the active master sits, not at the axis
            // default: opening a Bold master with the handle parked on
            // Regular means the first touch jumps the design.
            let here = project
                .master_locations
                .get(project.active)
                .and_then(|loc| loc.get(&axis.name).copied())
                .map(|normalized| {
                    runebender_core::document::var_model::denormalize_value(
                        normalized,
                        axis.min,
                        axis.default,
                        axis.max,
                    )
                })
                .unwrap_or(axis.default);
            let slider = cx.new(|_| {
                widgets::slider::SliderState::new()
                    .max(px32(axis.max))
                    .min(px32(axis.min))
                    .step(1.0)
                    .default_value(px32(here))
            });
            let axis_info = axis.clone();
            let sub = cx.subscribe_in(&slider, window, {
                move |this: &mut Self, _, event: &widgets::slider::SliderEvent, _window, cx| {
                    let widgets::slider::SliderEvent::Change(value) = event;
                    let raw = *value as f64;
                    let landed = {
                        let Some(project) = this.project.as_mut() else {
                            return;
                        };
                        project.location.insert(
                            axis_info.name.clone(),
                            runebender_core::document::var_model::normalize_value(
                                raw,
                                axis_info.min,
                                axis_info.default,
                                axis_info.max,
                            ),
                        );
                        project.master_at_location()
                    };
                    // Landing on a master hands editing back to it;
                    // anywhere else the canvas shows an instance.
                    if let Some(master) = landed {
                        this.switch_master(master);
                    }
                    cx.notify();
                }
            });
            self._subscriptions.push(sub);
            self.axis_sliders.push((i, slider));
        }
    }

    /// Park the preview (and the sliders) on a normalized location.
    /// Landing exactly on a master switches to it, the same contract
    /// as dragging a slider there.
    pub(crate) fn go_to_location(
        &mut self,
        target: &runebender_core::document::var_model::Location,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let landed = {
            let Some(project) = self.project.as_mut() else {
                return;
            };
            project.location = target.clone();
            project.master_at_location()
        };
        // Sliders show design coordinates; the location is normalized.
        let slider_values: Vec<(gpui::Entity<widgets::slider::SliderState>, f32)> = {
            let Some(project) = self.project.as_ref() else {
                return;
            };
            self.axis_sliders
                .iter()
                .filter_map(|(axis_index, slider)| {
                    let axis = project.axes.get(*axis_index)?;
                    let normalized = target.get(&axis.name).copied().unwrap_or(0.0);
                    let raw = runebender_core::document::var_model::denormalize_value(
                        normalized,
                        axis.min,
                        axis.default,
                        axis.max,
                    );
                    Some((slider.clone(), px32(raw)))
                })
                .collect()
        };
        for (slider, value) in slider_values {
            slider.update(cx, |st, cx| {
                st.set_value(value, window, cx);
            });
        }
        if let Some(master) = landed {
            self.switch_master(master);
        }
        cx.notify();
    }

    /// The preview's on/off switch, in the bottom bar's left corner
    /// where the tool hints used to be.
    pub(crate) fn preview_toggle(&self, cx: &mut Context<'_, Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .items_center()
            .gap_1()
            .flex_none()
            .child(
                div()
                    .id("preview-eye")
                    .flex_none()
                    .cursor_pointer()
                    .child(eye_icon(
                        if self.preview.visible {
                            t::text()
                        } else {
                            t::text_muted()
                        },
                        self.preview.visible,
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preview.visible = !this.preview.visible;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("preview-invert")
                    .flex_none()
                    .cursor_pointer()
                    .child(invert_icon(if self.preview.invert {
                        t::text()
                    } else {
                        t::text_muted()
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preview.invert = !this.preview.invert;
                        cx.notify();
                    })),
            )
    }

    /// What is left on the right of the bar: the blur, which is a
    /// spacing check. Show/hide and the ink flip live in the left
    /// corner beside each other.
    pub(crate) fn preview_controls(&self, cx: &mut Context<'_, Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_none()
            .child(div().text_color(t::text_muted()).child("blur"))
            .children(self.preview.blur_slider.as_ref().map(|slider| {
                // The thumb hangs past both ends of the track, so the
                // slider gets its own room rather than sitting on the
                // label.
                div().w(px(90.0)).mr_1().child(flat_slider(slider, cx))
            }))
    }

    /// Create the bottom bar's cell-size slider once a window exists.
    pub(crate) fn ensure_preview_slider(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.preview.blur_slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            widgets::slider::SliderState::new()
                .max(12.0)
                .min(0.0)
                .step(0.5)
                .default_value(0.0)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Self, _, event: &widgets::slider::SliderEvent, _window, cx| {
                let widgets::slider::SliderEvent::Change(value) = event;
                this.preview.blur = *value;
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.preview.blur_slider = Some(slider);
    }

    /// The strength control for model predictions.
    pub(crate) fn ensure_model_strength_slider(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.models.strength_slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            widgets::slider::SliderState::new()
                .min(0.25)
                .max(3.0)
                .step(0.05)
                .default_value(1.0)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Self, _, event: &widgets::slider::SliderEvent, _window, cx| {
                let widgets::slider::SliderEvent::Change(value) = event;
                this.models.strength = *value as f64;
                // The last judgement was made at the old strength.
                this.models.score = None;
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.models.strength_slider = Some(slider);
    }

    /// Creates the editor sidebar's mini-grid zoom slider once,
    /// 24 to 120 pixels in steps of 2.
    pub(crate) fn ensure_sidebar_slider(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.sidebar.slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            widgets::slider::SliderState::new()
                .max(120.0)
                .min(24.0)
                .step(2.0)
                .default_value(MINI_CELL)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Self, _, event: &widgets::slider::SliderEvent, _window, cx| {
                let widgets::slider::SliderEvent::Change(value) = event;
                this.sidebar.cell_size = *value;
                this.sidebar.scroll_row = 0;
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.sidebar.slider = Some(slider);
    }

    /// Creates the grid's cell zoom slider once, 48 to 200 pixels in
    /// steps of 4.
    pub(crate) fn ensure_cell_slider(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.grid.cell_slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            widgets::slider::SliderState::new()
                .max(200.0)
                .min(48.0)
                .step(4.0)
                .default_value(CELL)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Self, _, event: &widgets::slider::SliderEvent, _window, cx| {
                let widgets::slider::SliderEvent::Change(value) = event;
                this.grid.cell_size = *value;
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.grid.cell_slider = Some(slider);
    }

    /// The bar along the bottom: glyph add/remove, the selection count
    /// and cell zoom in grid mode; live readouts in the editor.
    pub(crate) fn status_bar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement + use<> {
        // Grid mode gets the Glyphs bottom bar: add/remove glyph on
        // the left, the selection count centered, cell zoom on the
        // right.
        if !matches!(self.mode, Mode::Editor(_)) && self.project.is_some() {
            let total = self.font().map(|f| f.glyphs.len()).unwrap_or(0);
            // The same list the grid draws, already filtered: counting
            // it again meant another pass over the whole font per frame.
            let shown = self.glyph_order().len();
            // The primary plus the multi-selection, counted once
            // when the primary is in both.
            let primary_name = self
                .selected
                .and_then(|i| self.font().and_then(|f| f.glyphs.get(i)))
                .map(|e| e.name.as_ref());
            let selected = self.grid.multi_selected.len()
                + usize::from(primary_name.is_some_and(|n| !self.grid.multi_selected.contains(n)));
            let center: SharedString = match &self.status_note {
                Some(note) => note.clone(),
                None => format!("{selected} selected · {shown}/{total} glyphs").into(),
            };
            let bar_button = |id: &'static str, mark: IconMark| {
                div()
                    .id(id)
                    .w(px(BAR_BUTTON))
                    .h(px(BAR_BUTTON))
                    .rounded(t::radius())
                    .border(t::stroke())
                    .border_color(t::cell_border())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(glyph_free_icon(t::cell_border(), t::stroke(), mark))
            };
            return div()
                .h(px(BOTTOM_BAR_H))
                .flex()
                .items_center()
                .gap_1()
                .px(px((BOTTOM_BAR_H - BAR_BUTTON) / 2.0))
                .bg(t::panel_bg())
                .border_t_1()
                .border_color(t::cell_border())
                .child(
                    bar_button("add-glyph", IconMark::Plus).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.command_add_glyph();
                            cx.notify();
                        },
                    )),
                )
                .child(
                    bar_button("remove-glyph", IconMark::Minus).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.command_remove_glyph();
                            cx.notify();
                        },
                    )),
                )
                .child(
                    div()
                        .flex_1()
                        .text_center()
                        .text_color(t::text_muted())
                        .child(center),
                )
                .child({
                    // Grid and List, as marks in the same boxes the
                    // add/remove buttons use; the current one is
                    // inverted, the way everything selected is.
                    let mode_button =
                        |id: &'static str,
                         mark: IconMark,
                         mode: FontViewMode,
                         current: FontViewMode,
                         cx: &mut Context<'_, Self>| {
                            let on = mode == current;
                            div()
                                .id(id)
                                .w(px(BAR_BUTTON))
                                .h(px(BAR_BUTTON))
                                .rounded(t::radius())
                                .border(t::stroke())
                                .border_color(if on {
                                    t::selected_bg()
                                } else {
                                    t::cell_border()
                                })
                                .when(on, |el| el.bg(t::selected_bg()))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .child(glyph_free_icon(
                                    if on {
                                        t::selected_ink()
                                    } else {
                                        t::cell_border()
                                    },
                                    t::stroke(),
                                    mark,
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.grid.view_mode = mode;
                                    cx.notify();
                                }))
                        };
                    let current = self.grid.view_mode;
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .mr_2()
                        .child(mode_button(
                            "view-grid",
                            IconMark::Grid,
                            FontViewMode::Grid,
                            current,
                            cx,
                        ))
                        .child(mode_button(
                            "view-list",
                            IconMark::List,
                            FontViewMode::List,
                            current,
                            cx,
                        ))
                })
                .children(
                    self.grid
                        .cell_slider
                        .as_ref()
                        .map(|slider| div().w(px(140.0)).child(flat_slider(slider, cx))),
                );
        }
        let text: SharedString = if let Some(note) = &self.status_note {
            note.clone()
        } else {
            match (&self.mode, self.selected, self.font()) {
                (Mode::Editor(_), _, Some(_)) => {
                    // No standing hint text here: the tool cheatsheet
                    // was permanent clutter. Only live readouts and
                    // transient notes speak.
                    if let Some(Drag::Measure { start, current }) = &self.editor.drag {
                        let (dx, dy) = (current.0 - start.0, current.1 - start.1);
                        let len = (dx * dx + dy * dy).sqrt();
                        let angle = dy.atan2(dx).to_degrees();
                        return div()
                            .px_4()
                            .py_1()
                            .bg(t::panel_bg())
                            .border_t_1()
                            .border_color(t::cell_border())
                            .text_color(t::text_muted())
                            .child(SharedString::from(format!(
                                "dx {dx:.0} · dy {dy:.0} · length {len:.1} · angle {angle:.1}°"
                            )));
                    }
                    SharedString::default()
                }
                (_, Some(i), Some(font)) => {
                    let g = &font.glyphs[i];
                    match g.codepoint {
                        Some(c) => {
                            format!("{} · U+{:04X} · advance {}", g.name, c as u32, g.advance)
                                .into()
                        }
                        None => format!("{} · unencoded · advance {}", g.name, g.advance).into(),
                    }
                }
                _ => "Click a glyph; double-click to edit · Cmd+O opens a font".into(),
            }
        };
        div()
            .h(px(BOTTOM_BAR_H))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(t::cell_border())
            .children(matches!(self.mode, Mode::Editor(_)).then(|| self.preview_toggle(cx)))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_color(t::text_muted())
                    .child(text),
            )
            .children(matches!(self.mode, Mode::Editor(_)).then(|| self.preview_controls(cx)))
    }
}
