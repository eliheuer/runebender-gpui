// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Wiring the fields to the font.
//!
//! `Workspace::new` is long because the editor has many fields, each
//! a widget the workspace owns plus a subscription that writes the
//! field's value through. Nothing here runs after the window opens;
//! what a field does lives in `edit/inspector.rs` and its siblings.

use crate::Arc;
use crate::Mode;
use crate::Mutex;
use crate::Workspace;
#[cfg(not(target_os = "macos"))]
use crate::actions::app_menus;
#[cfg(target_family = "wasm")]
#[cfg(target_family = "wasm")]
#[cfg(target_family = "wasm")]
#[cfg(target_family = "wasm")]
#[cfg(target_family = "wasm")]
use crate::platform::web_host;
use crate::widgets;
use crate::workspace::CELL;
use crate::workspace::EditorState;
use crate::workspace::FontInfoField;
use crate::workspace::FontInfoInputs;
use crate::workspace::FontViewMode;
use crate::workspace::GlyphInputs;
use crate::workspace::GridState;
use crate::workspace::InputFields;
use crate::workspace::KernInputs;
use crate::workspace::MINI_CELL;
use crate::workspace::MeasureOpts;
use crate::workspace::MetricField;
use crate::workspace::MetricInputs;
use crate::workspace::ModelsState;
use crate::workspace::PreviewState;
use crate::workspace::SidebarFilter;
use crate::workspace::SidebarState;
use gpui::AppContext;
use gpui::Context;
use gpui::SharedString;
use gpui::Window;
use gpui::px;
use kurbo::Affine;
use runebender_core::document::project::Project;
use std::collections::{HashMap, HashSet};
impl Workspace {
    /// Create the workspace for `project`, with every input widget
    /// wired. `load_error` is shown in the status bar when the project
    /// failed to open, and `start_mode` picks the grid or an editor.
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        project: Option<Project>,
        load_error: Option<SharedString>,
        start_mode: Mode,
    ) -> Self {
        #[cfg(not(target_os = "macos"))]
        let app_menu_bar = cx.new(|cx| widgets::menu_bar::MenuBar::new(app_menus(), cx));
        let search =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("Search glyphs"));
        let metric = |cx: &mut Context<'_, Self>, window: &mut Window| {
            cx.new(|cx| widgets::input::InputState::new(window, cx))
        };
        let width_input = metric(cx, window);
        let lsb_input = metric(cx, window);
        let rsb_input = metric(cx, window);
        let x_input = metric(cx, window);
        let y_input = metric(cx, window);
        let w_input = metric(cx, window);
        let h_input = metric(cx, window);
        let fi_family = metric(cx, window);
        let fi_style = metric(cx, window);
        let fi_upm = metric(cx, window);
        let fi_angle = metric(cx, window);
        let fi_asc = metric(cx, window);
        let fi_desc = metric(cx, window);
        let fi_xh = metric(cx, window);
        let fi_ch = metric(cx, window);
        let font_info_sub = |cx: &mut Context<'_, Self>,
                             window: &mut Window,
                             state: &gpui::Entity<widgets::input::InputState>,
                             which: FontInfoField| {
            let state = state.clone();
            cx.subscribe_in(&state, window, {
                let state = state.clone();
                move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                    if matches!(ev, widgets::input::InputEvent::PressEnter) {
                        let text = state.read(cx).value().to_string();
                        this.apply_font_info(which, &text);
                        this.rebuild_text_models();
                        this.refresh_font_info_inputs(true, window, cx);
                        cx.notify();
                    }
                }
            })
        };
        let sub_fi_family = font_info_sub(cx, window, &fi_family, FontInfoField::Family);
        let sub_fi_style = font_info_sub(cx, window, &fi_style, FontInfoField::Style);
        let sub_fi_upm = font_info_sub(cx, window, &fi_upm, FontInfoField::Upm);
        let sub_fi_angle = font_info_sub(cx, window, &fi_angle, FontInfoField::ItalicAngle);
        let sub_fi_asc = font_info_sub(cx, window, &fi_asc, FontInfoField::Ascender);
        let sub_fi_desc = font_info_sub(cx, window, &fi_desc, FontInfoField::Descender);
        let sub_fi_xh = font_info_sub(cx, window, &fi_xh, FontInfoField::XHeight);
        let sub_fi_ch = font_info_sub(cx, window, &fi_ch, FontInfoField::CapHeight);
        let fi_blues = metric(cx, window);
        let fi_oblues = metric(cx, window);
        let fi_stems_h = metric(cx, window);
        let fi_stems_v = metric(cx, window);
        let sub_fi_bv = font_info_sub(cx, window, &fi_blues, FontInfoField::BlueValues);
        let sub_fi_ob = font_info_sub(cx, window, &fi_oblues, FontInfoField::OtherBlues);
        let sub_fi_sh = font_info_sub(cx, window, &fi_stems_h, FontInfoField::StemsH);
        let sub_fi_sv = font_info_sub(cx, window, &fi_stems_v, FontInfoField::StemsV);
        let fi_typo_asc = metric(cx, window);
        let fi_typo_desc = metric(cx, window);
        let fi_typo_gap = metric(cx, window);
        let fi_hhea_asc = metric(cx, window);
        let fi_hhea_desc = metric(cx, window);
        let fi_hhea_gap = metric(cx, window);
        let fi_win_asc = metric(cx, window);
        let fi_win_desc = metric(cx, window);
        let sub_fi_ta = font_info_sub(cx, window, &fi_typo_asc, FontInfoField::TypoAscender);
        let sub_fi_td = font_info_sub(cx, window, &fi_typo_desc, FontInfoField::TypoDescender);
        let sub_fi_tg = font_info_sub(cx, window, &fi_typo_gap, FontInfoField::TypoLineGap);
        let sub_fi_ha = font_info_sub(cx, window, &fi_hhea_asc, FontInfoField::HheaAscender);
        let sub_fi_hd = font_info_sub(cx, window, &fi_hhea_desc, FontInfoField::HheaDescender);
        let sub_fi_hg = font_info_sub(cx, window, &fi_hhea_gap, FontInfoField::HheaLineGap);
        let sub_fi_wa = font_info_sub(cx, window, &fi_win_asc, FontInfoField::WinAscent);
        let sub_fi_wd = font_info_sub(cx, window, &fi_win_desc, FontInfoField::WinDescent);
        let kern_filter =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("Filter pairs"));
        let kern_first =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("First"));
        let kern_second =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("Second"));
        let kern_value =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("Value"));
        // The filter redraws the list as it changes; the
        // three editor fields commit together on Enter.
        let sub_kern_filter = cx.subscribe_in(
            &kern_filter,
            window,
            |_: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::Change) {
                    cx.notify();
                }
            },
        );
        let kern_commit =
            |cx: &mut Context<'_, Self>,
             window: &mut Window,
             state: &gpui::Entity<widgets::input::InputState>| {
                let state = state.clone();
                cx.subscribe_in(&state, window, {
                    move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                        if matches!(ev, widgets::input::InputEvent::PressEnter) {
                            let first = this.inputs.kern.first.read(cx).value().trim().to_string();
                            let second =
                                this.inputs.kern.second.read(cx).value().trim().to_string();
                            let value = this
                                .inputs
                                .kern
                                .value
                                .read(cx)
                                .value()
                                .trim()
                                .parse::<f64>();
                            if let (false, false, Ok(value)) =
                                (first.is_empty(), second.is_empty(), value)
                            {
                                this.apply_kern_pair(&first, &second, value);
                                cx.notify();
                            }
                        }
                    }
                })
            };
        let sub_kern_first = kern_commit(cx, window, &kern_first);
        let sub_kern_second = kern_commit(cx, window, &kern_second);
        let sub_kern_value = kern_commit(cx, window, &kern_value);
        let slant_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("Angle°"));
        let stroke_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("Width"));
        let sub_stroke = cx.subscribe_in(&stroke_input, window, {
            let state = stroke_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter)
                    && let Ok(width) = state.read(cx).value().trim().parse::<f64>()
                {
                    this.command_expand_stroke(width);
                    cx.notify();
                }
            }
        });
        let offset_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("±Units"));
        let sub_offset = cx.subscribe_in(&offset_input, window, {
            let state = offset_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter)
                    && let Ok(delta) = state.read(cx).value().trim().parse::<f64>()
                {
                    this.command_offset(delta);
                    cx.notify();
                }
            }
        });
        let fit_input = cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("%"));
        let sub_fit = cx.subscribe_in(&fit_input, window, {
            let state = fit_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter)
                    && let Ok(pct) = state
                        .read(cx)
                        .value()
                        .trim()
                        .trim_end_matches('%')
                        .parse::<f64>()
                {
                    this.command_fit_curve(pct / 100.0);
                    cx.notify();
                }
            }
        });
        let color_hex_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("#RRGGBB"));
        let sub_color_hex = cx.subscribe_in(&color_hex_input, window, {
            let state = color_hex_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    if this.command_add_palette_color(&text) {
                        state.update(cx, |st, cx| {
                            st.set_value(String::new(), window, cx);
                        });
                    }
                    cx.notify();
                }
            }
        });
        let ease_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("±50"));
        let sub_ease = cx.subscribe_in(&ease_input, window, {
            let state = ease_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter)
                    && let Ok(ease) = state.read(cx).value().trim().parse::<f64>()
                {
                    this.command_ease_interpolation(ease);
                    cx.notify();
                }
            }
        });
        let extrude_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("15,30"));
        let sub_extrude = cx.subscribe_in(&extrude_input, window, {
            let state = extrude_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    this.command_extrude(&text);
                    cx.notify();
                }
            }
        });
        let roughen_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("15,15,10"));
        let sub_roughen = cx.subscribe_in(&roughen_input, window, {
            let state = roughen_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    this.command_roughen(&text);
                    cx.notify();
                }
            }
        });
        let instance_name_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("Instance name"));
        let sub_instance_name = cx.subscribe_in(&instance_name_input, window, {
            let state = instance_name_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let name = state.read(cx).value().to_string();
                    this.command_instance_upsert(&name);
                    state.update(cx, |st, cx| {
                        st.set_value(String::new(), window, cx);
                    });
                    cx.notify();
                }
            }
        });
        let features_input = cx.new(|cx| widgets::input::InputState::new(window, cx).multi_line());
        let sub_features = cx.subscribe_in(
            &features_input,
            window,
            |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::Change) {
                    this.features_edited = true;
                    cx.notify();
                }
            },
        );
        let sub_slant = cx.subscribe_in(&slant_input, window, {
            let state = slant_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let Ok(angle) = state.read(cx).value().trim().parse::<f64>() else {
                        return;
                    };
                    if angle == 0.0 || angle.abs() >= 89.0 {
                        return;
                    }
                    // Positive leans right, the italic
                    // convention (Glyphs' Slant filter).
                    this.apply_transform(Affine::skew(angle.to_radians().tan(), 0.0));
                    cx.notify();
                }
            }
        });
        let metric_sub = |cx: &mut Context<'_, Self>,
                          window: &mut Window,
                          state: &gpui::Entity<widgets::input::InputState>,
                          which: MetricField| {
            let state = state.clone();
            cx.subscribe_in(&state, window, {
                let state = state.clone();
                move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                    if matches!(ev, widgets::input::InputEvent::PressEnter) {
                        let text = state.read(cx).value().to_string();
                        if let Ok(v) = text.trim().parse::<f64>() {
                            this.apply_metric(which, v);
                            this.rebuild_text_models();
                        }
                        this.refresh_metric_inputs(true, window, cx);
                        cx.notify();
                    }
                }
            })
        };
        let sub_w = metric_sub(cx, window, &width_input, MetricField::Width);
        let sub_l = metric_sub(cx, window, &lsb_input, MetricField::Lsb);
        let sub_r = metric_sub(cx, window, &rsb_input, MetricField::Rsb);
        let coord_sub = |cx: &mut Context<'_, Self>,
                         window: &mut Window,
                         state: &gpui::Entity<widgets::input::InputState>,
                         is_x: bool| {
            let state = state.clone();
            cx.subscribe_in(&state, window, {
                let state = state.clone();
                move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                    if matches!(ev, widgets::input::InputEvent::PressEnter) {
                        let text = state.read(cx).value().to_string();
                        if let Ok(v) = text.trim().parse::<f64>() {
                            this.apply_coord(is_x, v);
                        }
                        this.refresh_coord_inputs(true, window, cx);
                        cx.notify();
                    }
                }
            })
        };
        let sub_x = coord_sub(cx, window, &x_input, true);
        let sub_y = coord_sub(cx, window, &y_input, false);
        let size_sub = |cx: &mut Context<'_, Self>,
                        window: &mut Window,
                        state: &gpui::Entity<widgets::input::InputState>,
                        is_width: bool| {
            let state = state.clone();
            cx.subscribe_in(&state, window, {
                let state = state.clone();
                move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                    if matches!(ev, widgets::input::InputEvent::PressEnter) {
                        let text = state.read(cx).value().to_string();
                        if let Ok(v) = text.trim().parse::<f64>() {
                            this.apply_size(is_width, v);
                        }
                        this.refresh_coord_inputs(true, window, cx);
                        cx.notify();
                    }
                }
            })
        };
        let sub_sw = size_sub(cx, window, &w_input, true);
        let sub_sh = size_sub(cx, window, &h_input, false);
        let name_input = metric(cx, window);
        let unicode_input = metric(cx, window);
        let group_l_input = metric(cx, window);
        let group_r_input = metric(cx, window);
        // 0=name, 1=unicode, 2=left group, 3=right group.
        let glyph_sub = |cx: &mut Context<'_, Self>,
                         window: &mut Window,
                         state: &gpui::Entity<widgets::input::InputState>,
                         which: u8| {
            let state = state.clone();
            cx.subscribe_in(&state, window, {
                let state = state.clone();
                move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                    if matches!(ev, widgets::input::InputEvent::PressEnter) {
                        let text = state.read(cx).value().to_string();
                        match which {
                            0 => this.apply_glyph_rename(&text),
                            1 => this.apply_glyph_unicode(&text),
                            2 => this.apply_kern_group(true, &text),
                            4 => this.apply_glyph_note(&text),
                            5 => {
                                if let Ok(at) = text.trim().parse::<f64>() {
                                    this.command_add_shape_switch(at);
                                }
                            }
                            6 => this.apply_metrics_key(true, &text),
                            7 => this.apply_metrics_key(false, &text),
                            8 => this.apply_glyph_production(&text),
                            _ => this.apply_kern_group(false, &text),
                        }
                        this.refresh_glyph_inputs(true, window, cx);
                        cx.notify();
                    }
                }
            })
        };
        let component_name_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("glyph name"));
        let reference_glyph_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("glyph name"));
        let sub_ref = cx.subscribe_in(&reference_glyph_input, window, {
            let state = reference_glyph_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _window, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().trim().to_string();
                    this.reference_glyph = (!text.is_empty()).then_some(text);
                    cx.notify();
                }
            }
        });
        let anchor_name_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("anchor name"));
        let sub_anchor = cx.subscribe_in(&anchor_name_input, window, {
            let state = anchor_name_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _window, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    this.apply_anchor_name(&text);
                    cx.notify();
                }
            }
        });
        let corner_name_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("corner name"));
        let sub_corner = cx.subscribe_in(&corner_name_input, window, {
            let state = corner_name_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    let node = this.context_menu.as_ref().and_then(|m| m.start_point);
                    this.context_menu = None;
                    if let Some(node) = node {
                        this.command_apply_corner(node, text.trim());
                    }
                    state.update(cx, |st, cx| {
                        st.set_value(String::new(), window, cx);
                    });
                    cx.notify();
                }
            }
        });
        let smart_axis_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("Width,0,100"));
        let sub_smart_axis = cx.subscribe_in(&smart_axis_input, window, {
            let state = smart_axis_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    this.command_make_smart_axis(&text);
                    cx.notify();
                }
            }
        });
        let group_name_input = cx.new(|cx| {
            widgets::input::InputState::new(window, cx).placeholder("new group · o or |o")
        });
        let sub_group_name = cx.subscribe_in(&group_name_input, window, {
            let state = group_name_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    let trimmed = text.trim();
                    let (first_side, name) = match trimmed.strip_prefix('|') {
                        Some(rest) => (false, rest.trim()),
                        None => (true, trimmed),
                    };
                    if !name.is_empty() {
                        this.command_add_selection_to_group(first_side, name);
                        state.update(cx, |st, cx| {
                            st.set_value(String::new(), window, cx);
                        });
                    }
                    cx.notify();
                }
            }
        });
        let axis_map_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("400,430"));
        let sub_axis_map = cx.subscribe_in(&axis_map_input, window, {
            let state = axis_map_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    let mut parts = text.split(',').map(str::trim);
                    if let (Some(Ok(input)), Some(Ok(output))) = (
                        parts.next().map(str::parse::<f32>),
                        parts.next().map(str::parse::<f32>),
                    ) {
                        this.command_add_axis_mapping(input, output);
                        state.update(cx, |st, cx| {
                            st.set_value(String::new(), window, cx);
                        });
                    }
                    cx.notify();
                }
            }
        });
        let smart_value_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("value"));
        let sub_smart_value = cx.subscribe_in(&smart_value_input, window, {
            let state = smart_value_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().trim().to_string();
                    if !text.is_empty() {
                        this.command_set_smart_value(&text);
                        cx.notify();
                    }
                }
            }
        });
        let annotation_input =
            cx.new(|cx| widgets::input::InputState::new(window, cx).placeholder("note text"));
        let sub_note = cx.subscribe_in(&annotation_input, window, {
            let state = annotation_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    let at = this.context_menu.as_ref().map(|m| m.design);
                    this.context_menu = None;
                    if let (Some(at), false) = (at, text.trim().is_empty()) {
                        this.command_add_annotation(at, "note", text.trim());
                    }
                    state.update(cx, |st, cx| {
                        st.set_value(String::new(), window, cx);
                    });
                    cx.notify();
                }
            }
        });
        let sub_comp = cx.subscribe_in(&component_name_input, window, {
            let state = component_name_input.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, window, cx| {
                if matches!(ev, widgets::input::InputEvent::PressEnter) {
                    let text = state.read(cx).value().to_string();
                    this.commit_add_component(&text);
                    state.update(cx, |st, cx| {
                        st.set_value(String::new(), window, cx);
                    });
                    cx.notify();
                }
            }
        });
        let note_input = metric(cx, window);
        let switch_input = metric(cx, window);
        let lsb_key_input = metric(cx, window);
        let rsb_key_input = metric(cx, window);
        let sub_gn = glyph_sub(cx, window, &name_input, 0);
        let sub_gu = glyph_sub(cx, window, &unicode_input, 1);
        let sub_gl = glyph_sub(cx, window, &group_l_input, 2);
        let sub_gr = glyph_sub(cx, window, &group_r_input, 3);
        let sub_gnote = glyph_sub(cx, window, &note_input, 4);
        let sub_gswitch = glyph_sub(cx, window, &switch_input, 5);
        let sub_glk = glyph_sub(cx, window, &lsb_key_input, 6);
        let sub_grk = glyph_sub(cx, window, &rsb_key_input, 7);
        let production_input = metric(cx, window);
        let sub_gprod = glyph_sub(cx, window, &production_input, 8);
        let subscription = cx.subscribe_in(&search, window, {
            let search = search.clone();
            move |this: &mut Self, _, ev: &widgets::input::InputEvent, _window, cx| {
                if matches!(ev, widgets::input::InputEvent::Change) {
                    this.sidebar.search_query = search.read(cx).value().to_string().to_lowercase();
                    this.rebuild_search_regex();
                    // Fewer matches: start both grids at
                    // the top rather than past the end.
                    this.grid.scroll_row = 0;
                    this.sidebar.scroll_row = 0;
                    cx.notify();
                }
            }
        });
        let mut workspace = Self {
            project,
            load_error,
            selected: None,
            last_editor: None,
            sessions: Vec::new(),
            active_session: 0,
            nudging: false,
            mode: start_mode,
            editor: EditorState::new(),
            edit_buffer: runebender_core::text::buffer::TextBuffer::new(),
            collapsed_sections: HashSet::new(),
            reference_layers: HashSet::new(),
            show_all_masters: false,
            left_collapsed: false,
            #[cfg(not(target_os = "macos"))]
            app_menu_bar: app_menu_bar.clone(),
            focus_handle: cx.focus_handle(),
            status_note: None,
            last_save_label: None,
            context_menu: None,
            coord_quadrant: runebender_core::outline::path::Quadrant::default(),
            curve_comb: false,
            curve_continuity: false,
            measure_opts: MeasureOpts::default(),
            show_background: true,
            visible_glyph_layers: HashSet::default(),
            reference_glyph: None,
            glyph_image_cache: Arc::default(),
            color_selected: 0,
            show_color_preview: true,
            show_trajectories: false,
            hoi_live: None,
            shaping_focus: None,
            show_mark_cloud: false,
            feature_overrides: HashMap::default(),
            shaping_locale: None,
            roughen_seed: 0,
            features_edited: false,
            features_status: None,
            axis_sliders: Vec::new(),
            clipboard: Vec::new(),
            #[cfg(target_family = "wasm")]
            web_host: None,
            _watcher: None,
            last_save: Arc::new(Mutex::new(web_time::Instant::now())),
            _subscriptions: vec![
                subscription,
                sub_w,
                sub_l,
                sub_r,
                sub_x,
                sub_y,
                sub_gn,
                sub_gu,
                sub_gl,
                sub_gr,
                sub_gnote,
                sub_gswitch,
                sub_glk,
                sub_grk,
                sub_gprod,
                sub_comp,
                sub_corner,
                sub_note,
                sub_smart_axis,
                sub_smart_value,
                sub_group_name,
                sub_axis_map,
                sub_sw,
                sub_sh,
                sub_anchor,
                sub_ref,
                sub_fi_family,
                sub_fi_style,
                sub_fi_upm,
                sub_fi_angle,
                sub_fi_asc,
                sub_fi_desc,
                sub_fi_xh,
                sub_fi_ch,
                sub_fi_ta,
                sub_fi_td,
                sub_fi_tg,
                sub_fi_ha,
                sub_fi_hd,
                sub_fi_hg,
                sub_fi_wa,
                sub_fi_wd,
                sub_fi_bv,
                sub_fi_ob,
                sub_fi_sh,
                sub_fi_sv,
                sub_kern_filter,
                sub_kern_first,
                sub_kern_second,
                sub_kern_value,
                sub_slant,
                sub_features,
                sub_instance_name,
                sub_stroke,
                sub_offset,
                sub_fit,
                sub_color_hex,
                sub_ease,
                sub_extrude,
                sub_roughen,
            ],
            grid: GridState {
                sort_unicode: true,
                cell_size: CELL,
                viewport: gpui::size(px(0.0), px(0.0)),
                order: None,
                order_key: None,
                scroll_row: 0,
                cell_slider: None,
                multi_selected: HashSet::new(),
                view_mode: FontViewMode::Grid,
            },
            sidebar: SidebarState {
                filter: SidebarFilter::All,
                matches: None,
                counts: None,
                expanded_scripts: HashSet::new(),
                expanded_categories: HashSet::new(),
                viewport: gpui::size(px(0.0), px(0.0)),
                search_re: None,
                scroll_row: 0,
                tab: 0,
                cell_size: MINI_CELL,
                slider: None,
                search_input: search,
                search_query: String::new(),
                search_mode: 0,
                search_regex: false,
                search_case: false,
                search_predicates: None,
            },
            preview: PreviewState {
                visible: true,
                blur: 0.0,
                blur_cache: Arc::new(Mutex::new(None)),
                invert: false,
                blur_slider: None,
                sample_index: 0,
            },
            models: ModelsState {
                strength: 1.0,
                dir: None,
                summary: None,
                loaded: None,
                score: None,
                strength_slider: None,
            },
            inputs: InputFields {
                reference_glyph: reference_glyph_input.clone(),
                component_name: component_name_input.clone(),
                corner_name: corner_name_input.clone(),
                annotation: annotation_input.clone(),
                smart_axis: smart_axis_input,
                smart_value: smart_value_input,
                group_name: group_name_input,
                axis_map: axis_map_input,
                anchor_name: anchor_name_input.clone(),
                glyph: GlyphInputs {
                    name: name_input,
                    unicode: unicode_input,
                    group_l: group_l_input,
                    group_r: group_r_input,
                    note: note_input,
                    switch_at: switch_input,
                    lsb_key: lsb_key_input,
                    rsb_key: rsb_key_input,
                    production: production_input,
                },
                metric: MetricInputs {
                    width: width_input,
                    lsb: lsb_input,
                    rsb: rsb_input,
                    x: x_input,
                    y: y_input,
                    w: w_input,
                    h: h_input,
                },
                font_info: FontInfoInputs {
                    family: fi_family,
                    style: fi_style,
                    upm: fi_upm,
                    italic_angle: fi_angle,
                    ascender: fi_asc,
                    descender: fi_desc,
                    x_height: fi_xh,
                    cap_height: fi_ch,
                    typo_asc: fi_typo_asc,
                    typo_desc: fi_typo_desc,
                    typo_gap: fi_typo_gap,
                    hhea_asc: fi_hhea_asc,
                    hhea_desc: fi_hhea_desc,
                    hhea_gap: fi_hhea_gap,
                    win_asc: fi_win_asc,
                    win_desc: fi_win_desc,
                    blue_values: fi_blues,
                    other_blues: fi_oblues,
                    stems_h: fi_stems_h,
                    stems_v: fi_stems_v,
                },
                kern: KernInputs {
                    filter: kern_filter,
                    first: kern_first,
                    second: kern_second,
                    value: kern_value,
                },
                slant: slant_input,
                stroke: stroke_input,
                offset: offset_input,
                fit: fit_input,
                color_hex: color_hex_input,
                ease: ease_input,
                extrude: extrude_input,
                roughen: roughen_input,
                instance_name: instance_name_input,
                features: features_input,
            },
        };
        workspace.rebuild_text_models();
        workspace.start_watching(cx);
        #[cfg(target_family = "wasm")]
        if let Some(base) = web_host::server_from_location() {
            workspace.connect_web_host(base, cx);
        } else {
            workspace.status_note = Some(
                "Embedded demo font (read-only) · open with ?server=http://… to edit real fonts"
                    .into(),
            );
        }
        workspace
    }
}
