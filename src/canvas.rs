// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The canvas: the glyph grid and the editing view.
//!
//! These build what fills the middle of the window. Outlines are
//! painted through one canvas element over the whole grid rather than
//! one per cell, because gpui ends its render pass at every run of
//! paths and a canvas per cell meant a pass switch per cell.

use super::*;

impl Workspace {
    pub(crate) fn glyph_cell_sized(
        &self,
        index: usize,
        cell: f32,
        cell_h: f32,
        jump_on_click: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let font = self.font().unwrap();
        let entry = &font.glyphs[index];
        let name = entry.name.clone();
        let unicode_label: Option<SharedString> = entry
            .codepoint
            .map(|c| format!("U+{:04X}", c as u32).into());
        let detail_info: Option<SharedString> =
            (self.font_view_mode == FontViewMode::Detail && !jump_on_click).then(|| {
                let category = entry
                    .codepoint
                    .map(|c| {
                        runebender_core::category::GlyphCategory::from_codepoint(c).display_name()
                    })
                    .unwrap_or("Unencoded");
                format!("{category} · {:.0}", entry.advance).into()
            });
        let selected = if jump_on_click {
            matches!(self.mode, Mode::Editor(i) if i == index)
        } else {
            self.selected == Some(index) || self.multi_selected.contains(name.as_ref())
        };
        let labels = cell_label_metrics(cell);
        let (show_labels, label_px, label_h) = (labels.show, labels.size, labels.height);
        let incompatible = self
            .project
            .as_ref()
            .and_then(|p| p.compat.get(entry.name.as_ref()))
            .is_some_and(|ok| !ok);

        let paint = t::mark_paint(entry.mark.as_deref());
        let mark = paint.as_ref().map(|p| p.ink);
        let _ = font;
        div()
            .id(index)
            .w(px(cell))
            .h(px(cell_h))
            .flex()
            .flex_col()
            .bg(match (selected, paint.as_ref().and_then(|p| p.bg)) {
                (true, _) => t::cell_selected_bg(),
                (false, Some(fill)) => fill,
                (false, None) => t::cell_bg(),
            })
            .border(t::stroke())
            .border_color(if selected {
                t::cell_selected_ring()
            } else {
                paint
                    .as_ref()
                    .map(|p| p.border)
                    .unwrap_or_else(t::cell_border)
            })
            .rounded(t::radius_control())
            .cursor_pointer()
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                // Notes are transient: picking a glyph clears them so
                // the bottom bar's count shows again.
                this.status_note = None;
                if jump_on_click {
                    this.open_editor(index);
                } else {
                    let modifiers = event.modifiers();
                    if modifiers.platform {
                        // Cmd-click toggles membership.
                        this.grid_toggle_multi(index);
                    } else if modifiers.shift {
                        this.grid_extend_multi(index);
                    } else {
                        this.selected = Some(index);
                        this.multi_selected.clear();
                    }
                    if event.click_count() >= 2 {
                        this.open_editor(index);
                    }
                }
                cx.notify();
            }))
            // The outline itself is painted by one canvas over the
            // whole grid, not per cell: gpui ends its render pass at
            // every run of paths, so a canvas per cell meant a pass
            // switch per cell.
            .child(div().flex_1())
            .when(show_labels, |el| {
                el.child(
                    // Same inset left, right and bottom, a little air above,
                    // and the two lines close together (the web's
                    // cell-labels box).
                    div()
                        .h(px(label_h))
                        .pl(px(8.0))
                        .pr(px(8.0))
                        .pb(px(8.0))
                        .pt(px(4.0))
                        .flex()
                        .flex_col()
                        .justify_end()
                        .gap(px(2.0))
                        .text_size(px(label_px))
                        .line_height(px(labels.line))
                        .overflow_hidden()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_color(if selected {
                                    t::cell_selected_ring()
                                } else {
                                    mark.unwrap_or_else(t::text)
                                })
                                .when(incompatible, |el| {
                                    el.child(
                                        div().w(px(6.0)).h(px(6.0)).rounded_full().bg(t::anchor()),
                                    )
                                })
                                .child(SharedString::from(name)),
                        )
                        .when(labels.height >= 40.0, |el| {
                            el.child(
                                div()
                                    .text_color(if selected {
                                        t::cell_selected_ring()
                                    } else {
                                        mark.unwrap_or_else(t::text_muted)
                                    })
                                    .child(unicode_label.unwrap_or_else(|| "".into())),
                            )
                        })
                        // Detail mode's extra line: category and advance,
                        // the Glyphs 4 detail-grid info.
                        .when(detail_info.is_some(), |el| {
                            el.child(
                                div()
                                    .text_color(t::text_muted())
                                    .child(detail_info.clone().unwrap_or_default()),
                            )
                        }),
                )
            })
    }

    /// The List view: one row per glyph, one column per property —
    /// the Glyphs table. Click selects (cmd toggles, shift extends),
    /// double-click opens the editor; values are the active
    /// master's, edited through the Glyph panel, which already
    /// batch-edits a multi-selection.
    pub(crate) fn glyph_list_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        let order = self.glyph_order();
        const W_UNI: f32 = 68.0;
        const W_NUM: f32 = 52.0;
        const W_GROUP: f32 = 84.0;
        const W_CAT: f32 = 92.0;
        let head = |label: &'static str, w: f32| {
            div()
                .w(px(w))
                .flex_shrink_0()
                .text_xs()
                .text_color(t::text_muted())
                .child(label)
        };
        let mut list = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .px_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .border_b_1()
                    .border_color(t::panel_outline())
                    .child(div().w(px(14.0)).flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(80.0))
                            .text_xs()
                            .text_color(t::text_muted())
                            .child("Name"),
                    )
                    .child(head("Unicode", W_UNI))
                    .child(head("Width", W_NUM))
                    .child(head("LSB", W_NUM))
                    .child(head("RSB", W_NUM))
                    .child(head("Group L", W_GROUP))
                    .child(head("Group R", W_GROUP))
                    .child(head("Category", W_CAT)),
            );
        let mut rows = div()
            .id("glyph-list")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for &index in order.iter() {
            let entry = &font.glyphs[index];
            let name = entry.name.clone();
            let selected =
                self.selected == Some(index) || self.multi_selected.contains(name.as_ref());
            let mark = t::mark_paint(entry.mark.as_deref()).map(|p| p.ink);
            let ink = font.ink_bounds(index);
            let (lsb, rsb) = match ink {
                Some(r) => (
                    format!("{:.0}", r.x0),
                    format!("{:.0}", entry.advance - r.x1),
                ),
                None => (String::new(), String::new()),
            };
            let group = |left: bool| {
                runebender_core::glyph_ops::kern_group(&font.font, name.as_ref(), left)
                    .map(|g| {
                        g.as_str()
                            .replace("public.kern1.", "")
                            .replace("public.kern2.", "")
                    })
                    .unwrap_or_default()
            };
            let category = entry
                .codepoint
                .map(|c| {
                    runebender_core::category::GlyphCategory::from_codepoint(c)
                        .display_name()
                        .to_string()
                })
                .unwrap_or_else(|| "Unencoded".into());
            let text_color = if selected { t::text() } else { t::text_muted() };
            let cell = |value: String, w: f32| {
                div()
                    .w(px(w))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(text_color)
                    .overflow_hidden()
                    .child(value)
            };
            rows = rows.child(
                div()
                    .id(("glyph-row", index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(24.0))
                    .px_0p5()
                    .rounded(t::radius())
                    .when(selected, |el| el.bg(t::cell_selected_bg()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        this.status_note = None;
                        let modifiers = event.modifiers();
                        if modifiers.platform {
                            this.grid_toggle_multi(index);
                        } else if modifiers.shift {
                            this.grid_extend_multi(index);
                        } else {
                            this.selected = Some(index);
                            this.multi_selected.clear();
                        }
                        if event.click_count() >= 2 {
                            this.open_editor(index);
                        }
                        cx.notify();
                    }))
                    .child(
                        div().w(px(14.0)).flex_shrink_0().child(
                            div()
                                .w(px(9.0))
                                .h(px(9.0))
                                .rounded_full()
                                .bg(mark.unwrap_or(gpui::Rgba {
                                    r: 0.0,
                                    g: 0.0,
                                    b: 0.0,
                                    a: 0.0,
                                })),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(80.0))
                            .text_sm()
                            .text_color(if selected { t::accent() } else { t::text() })
                            .overflow_hidden()
                            .child(SharedString::from(name.clone())),
                    )
                    .child(cell(
                        entry
                            .codepoint
                            .map(|c| format!("U+{:04X}", c as u32))
                            .unwrap_or_default(),
                        W_UNI,
                    ))
                    .child(cell(format!("{:.0}", entry.advance), W_NUM))
                    .child(cell(lsb, W_NUM))
                    .child(cell(rsb, W_NUM))
                    .child(cell(group(true), W_GROUP))
                    .child(cell(group(false), W_GROUP))
                    .child(cell(category, W_CAT)),
            );
        }
        list.child(rows).into_any_element()
    }

    /// The positional-forms matrix (Counterpunch's Matrix Mode, the
    /// Arabic review surface): one row per base letter that carries
    /// positional variants, isol/init/medi/fina as columns, each a
    /// live thumbnail. Click a form to open it; a dash marks a
    /// missing form.
    pub(crate) fn glyph_matrix_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        // Families: base name → indices of [isol, init, medi, fina].
        let mut families: std::collections::BTreeMap<String, [Option<usize>; 4]> =
            std::collections::BTreeMap::new();
        for (i, entry) in font.glyphs.iter().enumerate() {
            let name = entry.name.as_ref();
            let (base, slot) = if let Some(b) = name.strip_suffix(".init") {
                (b, 1)
            } else if let Some(b) = name.strip_suffix(".medi") {
                (b, 2)
            } else if let Some(b) = name.strip_suffix(".fina") {
                (b, 3)
            } else {
                (name, 0)
            };
            let family = families.entry(base.to_string()).or_default();
            family[slot] = Some(i);
        }
        families.retain(|_, forms| forms[1..].iter().any(Option::is_some));
        if families.is_empty() {
            return div()
                .p_4()
                .text_sm()
                .text_color(t::text_muted())
                .child("No positional forms (.init/.medi/.fina) in this font")
                .into_any_element();
        }
        const THUMB: f32 = 56.0;
        let header = |label: &'static str| {
            div()
                .w(px(THUMB))
                .flex_shrink_0()
                .text_xs()
                .text_color(t::text_muted())
                .child(label)
        };
        let mut rows = div()
            .id("glyph-matrix")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px_2();
        rows = rows.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py_1()
                .border_b_1()
                .border_color(t::panel_outline())
                .child(
                    div()
                        .w(px(140.0))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(t::text_muted())
                        .child("Base"),
                )
                // RTL reading order: isolated at the right end would
                // be truer, but columns read left-to-right here with
                // the joining flow explicit in the labels.
                .child(header("isol"))
                .child(header("init"))
                .child(header("medi"))
                .child(header("fina")),
        );
        for (base, forms) in &families {
            let mut row = div().flex().items_center().gap_2().py_0p5().child(
                div()
                    .w(px(140.0))
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(t::text())
                    .overflow_hidden()
                    .child(base.clone()),
            );
            for (slot, form) in forms.iter().enumerate() {
                row = row.child(match *form {
                    Some(index) => {
                        let entry = &font.glyphs[index];
                        let (path, advance, asc, desc) = (
                            entry.path.clone(),
                            entry.advance,
                            font.ascender,
                            font.descender,
                        );
                        let selected = self.selected == Some(index);
                        div()
                            .id(("matrix-cell", index * 4 + slot))
                            .w(px(THUMB))
                            .h(px(THUMB))
                            .flex_shrink_0()
                            .rounded(t::radius())
                            .border(t::stroke())
                            .border_color(if selected {
                                t::cell_selected_ring()
                            } else {
                                t::cell_border()
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                                this.selected = Some(index);
                                this.multi_selected.clear();
                                if ev.click_count() >= 2 {
                                    this.open_editor(index);
                                }
                                cx.notify();
                            }))
                            .child(
                                canvas(
                                    move |bounds, _, _| bounds,
                                    move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                                        let h: f32 = bounds.size.height.into();
                                        let w: f32 = bounds.size.width.into();
                                        let em = (asc - desc).max(1.0);
                                        let scale =
                                            (h as f64 / em).min(w as f64 / advance.max(1.0));
                                        let ox = (w as f64 - advance * scale) / 2.0;
                                        let baseline = h as f64 + desc * scale;
                                        let view = Affine::translate((ox, baseline))
                                            * Affine::scale_non_uniform(scale, -scale);
                                        if let Some(p) = build_fill_path(&path, view, bounds.origin)
                                        {
                                            window.paint_path(p, t::glyph_fill());
                                        }
                                    },
                                )
                                .size_full(),
                            )
                            .into_any_element()
                    }
                    None => div()
                        .w(px(THUMB))
                        .h(px(THUMB))
                        .flex_shrink_0()
                        .rounded(t::radius())
                        .border(t::stroke())
                        .border_color(t::panel_outline())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(t::text_muted())
                        .child("–")
                        .into_any_element(),
                });
            }
            rows = rows.child(row);
        }
        rows.into_any_element()
    }

    pub(crate) fn editor_view(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        // The glyph's background image (tracing template), with its
        // placement rect in design space. Shear in the stored
        // transform is not drawn; scale and offset are.
        let glyph_image: Option<(Arc<gpui::RenderImage>, kurbo::Rect)> = (self.show_background)
            .then(|| {
                let img = self
                    .font()?
                    .font
                    .get_glyph(self.font()?.glyphs.get(index)?.name.as_ref())?
                    .image
                    .clone()?;
                let file = img.file_name().to_string_lossy().to_string();
                let image = self.glyph_image(&file)?;
                let size = image.size(0);
                let (w, h) = (i32::from(size.width) as f64, i32::from(size.height) as f64);
                let t = &img.transform;
                let rect = kurbo::Rect::new(
                    t.x_offset,
                    t.y_offset,
                    t.x_offset + w * t.x_scale,
                    t.y_offset + h * t.y_scale,
                );
                Some((image, rect))
            })
            .flatten();
        let font = self.font().unwrap();
        let entry = &font.glyphs[index];
        let outline = entry.contour_path.clone();
        let component_path = entry.component_path.clone();
        let component_names = entry.component_names.clone();
        // The text buffer, web-style: every sort's fill (the active
        // one too while the text tool is up), its quiet metric box,
        // corner marks (kern-colored during a kern drag), and the
        // caret. Coordinates are relative to the active sort.
        struct SortPaint {
            path: Option<Arc<BezPath>>,
            x: f64,
            y: f64,
            advance: f64,
            active: bool,
            /// 0 = normal, 1 = kern-active, 2 = kern-previous.
            kern: u8,
        }
        let text_mode = self.editor.tool == Tool::Text;
        let (sort_paints, text_caret): (Vec<SortPaint>, Option<(f64, f64)>) = {
            let line_height = self.text_line_height();
            let layout = self.edit_buffer.layout(line_height);
            let active = self.edit_buffer.active_sort();
            let kern_sort = self.edit_buffer.manual_kerning_sort();
            let off = self.editor.sort_offset;
            let paints = layout
                .items
                .iter()
                .filter_map(|item| {
                    let sort = self.edit_buffer.sort(item.index)?;
                    if sort.is_absorbed() {
                        return None;
                    }
                    let is_active = Some(item.index) == active;
                    let path = sort
                        .glyph_name()
                        .and_then(|n| font.name_map.get(n))
                        .map(|&g| font.glyphs[g].path.clone());
                    Some(SortPaint {
                        path,
                        x: item.x - off.0,
                        y: item.y - off.1,
                        advance: item.advance_width,
                        active: is_active,
                        kern: match kern_sort {
                            Some(k) if k == item.index => 1,
                            Some(k) if k == item.index + 1 => 2,
                            _ => 0,
                        },
                    })
                })
                .collect();
            let caret = text_mode.then(|| (layout.cursor_x - off.0, layout.cursor_y - off.1));
            (paints, caret)
        };
        let (sort_top, sort_bottom) = self.text_sort_bounds();

        // Masters toggled visible in the Layers section, drawn as dim
        // reference underlays.
        let reference_paths: Vec<Arc<BezPath>> = self
            .project
            .as_ref()
            .map(|p| {
                let shown: Vec<usize> = if self.show_all_masters {
                    (0..p.masters.len()).collect()
                } else {
                    self.reference_layers.iter().copied().collect()
                };
                shown
                    .iter()
                    .filter(|&&i| i != p.active && i < p.masters.len())
                    .filter_map(|&i| {
                        p.masters[i]
                            .glyphs
                            .iter()
                            .find(|g| g.name == entry.name)
                            .map(|g| g.path.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Between masters the sliders describe an instance: the web
        // swaps the outline for the interpolated one and marks the
        // view read-only, rather than ghosting it behind an editable
        // master, which leaves you editing something you cannot see.
        let showing_instance = self.project.as_ref().is_some_and(|p| p.showing_instance());
        let instance: Option<Arc<BezPath>> = showing_instance
            .then(|| {
                self.project
                    .as_ref()
                    .and_then(|p| p.interpolated_glyph(entry.name.as_ref()))
                    .map(|(path, _)| Arc::new(path))
            })
            .flatten();
        let ghost: Option<Arc<BezPath>> = None;
        let outline = instance.clone().unwrap_or(outline);
        let points = entry.points.clone();
        // Where each closed contour starts and which way it runs, for
        // the start arrow. Open contours (pen paths in progress) get
        // none, like the web.
        let start_markers: Vec<((f64, f64), (f64, f64), bool)> = font
            .font
            .get_glyph(entry.name.as_ref())
            .map(|g| {
                g.contours
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        c.points
                            .first()
                            .is_none_or(|p| p.typ != norad::PointType::Move)
                    })
                    .filter_map(|(ci, _)| {
                        let mut here = entry.points.iter().filter(|p| p.contour == ci).peekable();
                        let all: Vec<&GlyphPoint> = here.by_ref().collect();
                        let first = all.iter().position(|p| p.on_curve)?;
                        let start = all[first];
                        let next = all[(first + 1) % all.len()];
                        Some((
                            (start.x, start.y),
                            (next.x, next.y),
                            self.editor.selected.contains(&(start.contour, start.index)),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let anchors = entry.anchors.clone();
        let selected_anchors = self.editor.selected_anchors.clone();
        let advance = entry.advance;
        let ascender = font.ascender;
        let descender = font.descender;
        let upm = font.units_per_em;
        let x_height = font.x_height;
        let cap_height = font.cap_height;
        // Alignment zones (postscript blues, position pairs), drawn
        // as quiet bands like Glyphs' beige zones.
        let zones: Vec<(f64, f64)> = {
            let info = &font.font.font_info;
            info.postscript_blue_values
                .iter()
                .flatten()
                .chain(info.postscript_other_blues.iter().flatten())
                .copied()
                .collect::<Vec<f64>>()
                .chunks_exact(2)
                .map(|pair| (pair[0].min(pair[1]), pair[0].max(pair[1])))
                .collect()
        };
        // Node trajectories across the axis (HOI view): sampled at
        // equal axis stops, so dot spacing reads as velocity, and
        // brace layers visibly bend the paths.
        let trajectories: Option<Vec<Vec<kurbo::Point>>> = self
            .show_trajectories
            .then(|| {
                self.project
                    .as_ref()
                    .and_then(|p| p.trajectory_samples(entry.name.as_ref(), 10))
            })
            .flatten();
        // The mark cloud: every mark whose _anchor matches one of
        // this glyph's anchors, ghosted in place — the crowding
        // check while positioning anchors.
        let mark_cloud: Vec<Arc<BezPath>> = if self.show_mark_cloud {
            let mut placed = Vec::new();
            'outer: for candidate in font.glyphs.iter() {
                for (mark_anchor, mx, my) in candidate.anchors.iter() {
                    let Some(base_name) = mark_anchor.strip_prefix('_') else {
                        continue;
                    };
                    let Some((_, ax, ay)) = entry
                        .anchors
                        .iter()
                        .find(|(name, _, _)| name.as_ref() == base_name)
                    else {
                        continue;
                    };
                    if candidate.path.elements().is_empty() {
                        continue;
                    }
                    placed.push(Arc::new(
                        Affine::translate((ax - mx, ay - my)) * candidate.path.as_ref().clone(),
                    ));
                    if placed.len() >= 60 {
                        break 'outer;
                    }
                    continue 'outer;
                }
            }
            placed
        } else {
            Vec::new()
        };
        // Mask contours: drawn in the accent as a warning, and cut
        // out of the space-hold preview fill.
        let mask_paths: Vec<Arc<BezPath>> = font
            .font
            .get_glyph(entry.name.as_ref())
            .map(|g| {
                read_masks(g)
                    .into_iter()
                    .filter_map(|ci| {
                        g.contours
                            .get(ci)
                            .map(|c| Arc::new(runebender_core::glyph_paths::contour_to_bezpath(c)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Annotations: working marks pinned to design-space points.
        let annotations: Vec<Annotation> = font
            .font
            .get_glyph(entry.name.as_ref())
            .map(read_annotations)
            .unwrap_or_default();
        // HOI knobs (one per node, at its intermediate point or the
        // linear middle) and the live curve while one is dragged.
        let hoi_knobs: Vec<((usize, usize), (f64, f64))> = (self.show_trajectories)
            .then(|| {
                self.project.as_ref().and_then(|p| {
                    let (lo, hi) = p.axis_end_masters()?;
                    let name = entry.name.as_ref();
                    let a = p.masters[lo].font.get_glyph(name)?;
                    let b = p.masters[hi].font.get_glyph(name)?;
                    let curves = read_hoi_intermediates(a);
                    let mut knobs = Vec::new();
                    for (ci, (ca, cb)) in a.contours.iter().zip(b.contours.iter()).enumerate() {
                        for (pi, (pa, pb)) in ca.points.iter().zip(cb.points.iter()).enumerate() {
                            let q = curves
                                .get(&(ci, pi))
                                .copied()
                                .unwrap_or(((pa.x + pb.x) / 2.0, (pa.y + pb.y) / 2.0));
                            knobs.push(((ci, pi), q));
                        }
                    }
                    Some(knobs)
                })
            })
            .flatten()
            .unwrap_or_default();
        let hoi_live = self.hoi_live;
        let hoi_drag_ends: Option<((f64, f64), (f64, f64))> = match &self.editor.drag {
            Some(Drag::HoiKnob { a, b, .. }) => Some((*a, *b)),
            _ => None,
        };
        // Guides, drawn across the whole canvas under the outline:
        // the master's global fontinfo guidelines plus the open
        // glyph's own. The hot one (hovered or mid-drag) draws
        // brighter, with its knob grown.
        let guide_hot: Option<(bool, usize)> = match &self.editor.drag {
            Some(Drag::Guide { local, index }) => Some((*local, *index)),
            _ => self.editor.guide_hover,
        };
        let guides: Vec<(bool, norad::Line)> = font
            .font
            .font_info
            .guidelines
            .iter()
            .flatten()
            .map(|g| (false, g.line))
            .chain(
                font.font
                    .get_glyph(entry.name.as_ref())
                    .into_iter()
                    .flat_map(|g| g.guidelines.iter())
                    .map(|g| (true, g.line)),
            )
            .collect();
        // The metric box runs to the upm when that is higher than the
        // ascender, so an icon font's full em still reads as its space
        // (web `glyph_metric_bounds`).
        let box_top = upm.max(ascender);
        let box_bottom = descender;

        let transform = self.editor.transform();
        let zoom = self.editor.zoom();
        let selected_points = self.editor.selected.clone();
        let locked_points = self.editor.locked_points.clone();
        let marquee = match &self.editor.drag {
            Some(Drag::Marquee { start, current, .. }) => Some((*start, *current)),
            _ => None,
        };
        // Free-transform box: shown for a multi-point selection with
        // the select tool up, and kept up during its own drag.
        let transform_box: Option<kurbo::Rect> = (self.editor.tool == Tool::Select
            && !matches!(self.editor.drag, Some(Drag::Marquee { .. })))
        .then(|| self.selection_bbox(index))
        .flatten();
        let shape_preview = match &self.editor.drag {
            Some(Drag::Shape { start, current }) => {
                Some((*start, *current, self.editor.shape_ellipse))
            }
            _ => None,
        };
        let measure_line = match &self.editor.drag {
            Some(Drag::Measure { start, current }) => Some((*start, *current)),
            _ => None,
        };
        // Curve overlays: comb strips and continuity rings, computed
        // in design space from the shared analyses in core.
        let comb_strips: Vec<Vec<runebender_core::curve::CombSample>> =
            if self.curve_comb && self.editor.tool != Tool::Preview {
                font.font
                    .get_glyph(entry.name.as_ref())
                    .map(|g| {
                        let cubics = runebender_core::curve::cubics_from_norad(g);
                        let maxk = runebender_core::curve::max_curvature(&cubics);
                        if maxk <= 1e-12 {
                            (Vec::new(), 0.0)
                        } else {
                            (
                                runebender_core::curve::curvature_comb(
                                    &cubics,
                                    1.0,
                                    74.0 / maxk,
                                    false,
                                    16,
                                ),
                                maxk,
                            )
                        }
                    })
                    .map(|(strips, _)| strips)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        let comb_maxk: f64 = comb_strips
            .iter()
            .flat_map(|s| s.iter())
            .map(|s| s.kappa.abs())
            .fold(0.0, f64::max);
        let continuity_rings: Vec<(kurbo::Point, gpui::Rgba)> =
            if self.curve_continuity && self.editor.tool != Tool::Preview {
                font.font
                    .get_glyph(entry.name.as_ref())
                    .map(|g| {
                        let cubics = runebender_core::curve::cubics_from_norad(g);
                        runebender_core::curve::node_continuity(&cubics)
                            .into_iter()
                            .filter_map(|nc| {
                                use runebender_core::curve::GLevel;
                                let color = match nc.level {
                                    GLevel::Corner => return None,
                                    GLevel::G2 | GLevel::G3 => t::continuity_g2(),
                                    GLevel::G1 => t::continuity_g1(),
                                    GLevel::G1Line => t::continuity_line(),
                                    GLevel::Kink => t::continuity_kink(),
                                };
                                Some((nc.at, color))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        // Measure-tool HUD: colorized strokes, measurements, and side
        // bearings from core's measure module, in design space. The
        // paint closure maps them to the screen and draws dimension
        // lines + labels.
        let measure_opts = self.measure_opts;
        // Every segment's own bounding box, for the size labels.
        let segment_boxes: Vec<kurbo::Rect> = if self.measure_opts.sizes {
            use kurbo::Shape as _;
            font.font
                .get_glyph(entry.name.as_ref())
                .map(|g| {
                    runebender_core::segment_ops::segments(g)
                        .into_iter()
                        .map(|hit| hit.seg.bounding_box())
                        .filter(|b| b.width() >= 1.0 || b.height() >= 1.0)
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let measure_hud: Option<(
            Vec<runebender_core::measure::ColoredStroke>,
            Vec<runebender_core::measure::Measurement>,
            Option<runebender_core::measure::SideBearings>,
        )> = if measure_opts.any() && self.editor.tool != Tool::Preview {
            font.font.get_glyph(entry.name.as_ref()).map(|g| {
                use runebender_core::measure;
                use runebender_core::model::workspace::Contour as WContour;
                let paths: Vec<runebender_core::path::Path> = g
                    .contours
                    .iter()
                    .map(|c| runebender_core::path::Path::from_contour(&WContour::from_norad(c)))
                    .collect();
                let strokes = if measure_opts.colorize {
                    measure::colored_strokes(&paths)
                } else {
                    Vec::new()
                };
                let measurements =
                    if measure_opts.handles || measure_opts.segments || measure_opts.spans {
                        measure::glyph_measurements(&paths)
                    } else {
                        Vec::new()
                    };
                let sb = (measure_opts.sidebearings && g.width > 0.0)
                    .then(|| measure::side_bearings(&paths, g.width))
                    .flatten();
                (strokes, measurements, sb)
            })
        } else {
            None
        };
        // Background layer outline + reference glyph ghost.
        let background_path: Option<Arc<BezPath>> = self
            .show_background
            .then(|| {
                Self::background_layer_name(&font.font).and_then(|layer| {
                    font.font
                        .layers
                        .get(&layer)
                        .and_then(|l| l.get_glyph(entry.name.as_ref()))
                        .map(|g| Arc::new(runebender_core::glyph_paths::contours_to_bezpath(g)))
                })
            })
            .flatten();
        // Stacked color layers (COLRv0 preview): each mapped layer's
        // copy of this glyph filled with its palette color, bottom
        // first, under the editing outline.
        let color_preview: Vec<(Arc<BezPath>, gpui::Rgba)> = if self.show_color_preview {
            let palette = read_color_palette(&font.font);
            read_color_mapping(&font.font)
                .into_iter()
                .filter_map(|(layer, color)| {
                    let c = palette.get(color)?;
                    let glyph = font
                        .font
                        .layers
                        .get(&layer)?
                        .get_glyph(entry.name.as_ref())?;
                    Some((
                        Arc::new(runebender_core::glyph_paths::contours_to_bezpath(glyph)),
                        gpui::Rgba {
                            r: c[0] as f32,
                            g: c[1] as f32,
                            b: c[2] as f32,
                            a: c[3] as f32,
                        },
                    ))
                })
                .collect()
        } else {
            Vec::new()
        };
        // Visible per-glyph layers, drawn like the background.
        let glyph_layer_paths: Vec<Arc<BezPath>> = font
            .font
            .layers
            .iter()
            .filter(|l| !l.is_default() && self.visible_glyph_layers.contains(l.name().as_str()))
            .filter_map(|l| l.get_glyph(entry.name.as_ref()))
            .map(|g| Arc::new(runebender_core::glyph_paths::contours_to_bezpath(g)))
            .collect();
        let reference_path: Option<Arc<BezPath>> = self
            .reference_glyph
            .as_ref()
            .and_then(|name| font.name_map.get(name))
            .map(|&g| font.glyphs[g].path.clone());
        // Alt-hover segment highlight (select tool).
        let hover_seg = self.editor.segment_hover;
        // Sidebearing edge under the pointer (or mid-drag).
        let sidebearing_hover = self.editor.sidebearing_hover.or(match &self.editor.drag {
            Some(Drag::Sidebearing { right, .. }) => Some(*right),
            _ => None,
        });
        let component_selected = self.editor.selected_component.is_some();
        // Pen rubber band: last on-curve of the open contour to the
        // pointer, with a ring on the start point when close would
        // land (web PenPreview).
        let pen_preview: Option<((f64, f64), (f64, f64), Option<(f64, f64)>)> = (|| {
            let contour = self
                .editor
                .pen
                .as_ref()
                .map(|p| p.contour)
                .or(self.editor.hyper_contour)?;
            let pointer = self.editor.pointer?;
            let (px_, py_) = self.editor.window_to_design(pointer);
            let glyph = font.font.get_glyph(entry.name.as_ref())?;
            let points = &glyph.contours.get(contour)?.points;
            let last = points
                .iter()
                .rev()
                .find(|p| p.typ != norad::PointType::OffCurve)?;
            let start = points.first()?;
            let close_radius = HIT_RADIUS_PX / self.editor.zoom();
            let close = (points.len() >= 3
                && ((start.x - px_).powi(2) + (start.y - py_).powi(2)).sqrt() <= close_radius)
                .then_some((start.x, start.y));
            Some(((last.x, last.y), (px_, py_), close))
        })();

        // Knife drag: the cut line plus its contour intersections.
        let knife_line: Option<((f64, f64), (f64, f64), Vec<kurbo::Point>)> =
            match &self.editor.drag {
                Some(Drag::Knife { start, current }) => {
                    let hits = font
                        .font
                        .get_glyph(entry.name.as_ref())
                        .map(|g| {
                            runebender_core::knife::knife_hit_points(
                                g,
                                kurbo::Point::new(start.0, start.1),
                                kurbo::Point::new(current.0, current.1),
                            )
                        })
                        .unwrap_or_default();
                    Some((*start, *current, hits))
                }
                _ => None,
            };
        // An instance draws like Preview: filled, no editable chrome.
        let preview_mode = self.editor.tool == Tool::Preview || showing_instance;
        let bounds_slot = self.editor.bounds.clone();
        let needs_fit = !self.editor.initialized;

        div()
            .flex_1()
            .relative()
            .children(self.context_menu_overlay(cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.editor_mouse_down(
                        event.position,
                        event.modifiers.shift,
                        event.modifiers.alt,
                        event.click_count,
                    );
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    if event.pressed_button == Some(MouseButton::Left) {
                        if this.editor_mouse_drag(
                            event.position,
                            event.modifiers.shift,
                            event.modifiers.alt,
                        ) {
                            cx.notify();
                        }
                    } else if this.editor_hover(event.position, event.modifiers.alt) {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseUpEvent, _, cx| {
                    this.editor_mouse_up();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.editor_context_menu(event.position);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    this.editor_scroll(event);
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    move |bounds, _, _| bounds,
                    move |_, bounds: Bounds<gpui::Pixels>, window, cx| {
                        *bounds_slot.lock().unwrap() = bounds;
                        // Everything the editor draws is clipped to
                        // the canvas: without a mask the outline and
                        // the neighbouring sorts paint straight over
                        // the header and the panels beside it.
                        window.with_content_mask(
                            Some(gpui::ContentMask { bounds }),
                            move |window| {
                                let mut transform = transform;
                                let mut zoom = zoom;
                                if needs_fit {
                                    // First paint after opening: fit the glyph.
                                    // Recompute locally; the entity state is
                                    // fitted on the next mouse interaction via
                                    // the same bounds slot.
                                    let h: f32 = bounds.size.height.into();
                                    let w: f32 = bounds.size.width.into();
                                    let mut vp = ViewPort::new();
                                    vp.fit_to_canvas(
                                        w as f64, h as f64, advance, ascender, descender, 0.62,
                                    );
                                    transform = vp.affine();
                                    zoom = vp.zoom;
                                }
                                let _ = cx;
                                let origin = bounds.origin;
                                let to_screen = |x: f64, y: f64| {
                                    let p = transform * kurbo::Point::new(x, y);
                                    gpui::point(
                                        origin.x + px(p.x as f32),
                                        origin.y + px(p.y as f32),
                                    )
                                };

                                // Zoom-dependent design grid behind everything
                                // (web draw_design_grid): the 8-unit lattice
                                // fades in past 0.8x, and past 8x a 2-unit fine
                                // grid joins underneath — the 8s stay one grid
                                // at every zoom. Anchored at the active sort's
                                // origin (our design space is sort-relative),
                                // so the baseline lands on a gridline.
                                let smoothstep = |t: f64| t * t * (3.0 - 2.0 * t);
                                let grid_mid_alpha =
                                    smoothstep(((zoom - 0.8) / 0.8).clamp(0.0, 1.0));
                                let grid_close_alpha =
                                    smoothstep(((zoom - 8.0) / 8.0).clamp(0.0, 1.0));
                                if !preview_mode && grid_mid_alpha > 0.0 {
                                    let inv = transform.inverse();
                                    let bw: f32 = bounds.size.width.into();
                                    let bh: f32 = bounds.size.height.into();
                                    let c0 = inv * kurbo::Point::new(0.0, 0.0);
                                    let c1 = inv * kurbo::Point::new(bw as f64, bh as f64);
                                    let (min_x, max_x) = (c0.x.min(c1.x), c0.x.max(c1.x));
                                    let (min_y, max_y) = (c0.y.min(c1.y), c0.y.max(c1.y));
                                    let level = |spacing: f64,
                                                 skip_every: u64,
                                                 width_px: f32,
                                                 color: gpui::Rgba,
                                                 window: &mut Window| {
                                    let mut pb = PathBuilder::stroke(px(width_px));
                                    for ix in (min_x / spacing).floor() as i64
                                        ..=(max_x / spacing).ceil() as i64
                                    {
                                        if skip_every > 0
                                            && ix.unsigned_abs() % skip_every == 0
                                        {
                                            continue;
                                        }
                                        let x = ix as f64 * spacing;
                                        pb.move_to(to_screen(x, min_y));
                                        pb.line_to(to_screen(x, max_y));
                                    }
                                    for iy in (min_y / spacing).floor() as i64
                                        ..=(max_y / spacing).ceil() as i64
                                    {
                                        if skip_every > 0
                                            && iy.unsigned_abs() % skip_every == 0
                                        {
                                            continue;
                                        }
                                        let y = iy as f64 * spacing;
                                        pb.move_to(to_screen(min_x, y));
                                        pb.line_to(to_screen(max_x, y));
                                    }
                                    if let Ok(p) = pb.build() {
                                        window.paint_path(p, color);
                                    }
                                };
                                    level(
                                        8.0,
                                        0,
                                        1.0,
                                        t::design_grid_coarse(grid_mid_alpha as f32),
                                        window,
                                    );
                                    let close_alpha =
                                        smoothstep(((zoom - 8.0) / 8.0).clamp(0.0, 1.0));
                                    if close_alpha > 0.0 {
                                        // The 2s only; every 4th line is an 8
                                        // the mid pass already drew.
                                        level(
                                            2.0,
                                            4,
                                            0.5,
                                            t::design_grid_fine(close_alpha as f32),
                                            window,
                                        );
                                    }
                                }

                                // Metrics: baseline, ascender, descender,
                                // sidebearings.
                                let hline = |y: f64, window: &mut Window| {
                                    let a = to_screen(0.0, y);
                                    let b = to_screen(advance, y);
                                    window.paint_quad(gpui::fill(
                                        Bounds::from_corners(a, gpui::point(b.x, b.y + px(1.0))),
                                        t::metrics_line(),
                                    ));
                                };
                                if !text_mode {
                                    // The tracing template sits under
                                    // everything.
                                    if let Some((image, rect)) = &glyph_image {
                                        let a = to_screen(rect.x0, rect.y0);
                                        let b = to_screen(rect.x1, rect.y1);
                                        let target = Bounds::from_corners(
                                            gpui::point(a.x.min(b.x), a.y.min(b.y)),
                                            gpui::point(a.x.max(b.x), a.y.max(b.y)),
                                        );
                                        let _ = window.paint_image(
                                            target,
                                            target,
                                            gpui::Corners::default(),
                                            image.clone(),
                                            0,
                                            true,
                                        );
                                    }
                                    // Alignment zone bands.
                                    for &(lo, hi) in &zones {
                                        let a = to_screen(0.0, hi);
                                        let b = to_screen(0.0, lo);
                                        window.paint_quad(gpui::fill(
                                            Bounds::from_corners(
                                                gpui::point(bounds.origin.x, a.y),
                                                gpui::point(
                                                    bounds.origin.x + bounds.size.width,
                                                    b.y,
                                                ),
                                            ),
                                            t::zone_band(),
                                        ));
                                    }
                                    // The color stack, bottom first, so
                                    // editing happens over the composite.
                                    for (path, color) in &color_preview {
                                        if let Some(p) = build_fill_path(path, transform, origin) {
                                            window.paint_path(p, *color);
                                        }
                                    }
                                    // Every guide the font defines, the way
                                    // the web draws them: the baseline
                                    // always, then the box edges, the upm,
                                    // ascender, descender, x-height and
                                    // cap-height, deduplicated.
                                    let mut ys =
                                        vec![0.0, box_top, box_bottom, upm, ascender, descender];
                                    ys.extend(x_height);
                                    ys.extend(cap_height);
                                    ys.retain(|y: &f64| y.is_finite());
                                    ys.sort_by(|a, b| a.total_cmp(b));
                                    ys.dedup_by(|a, b| (*a - *b).abs() < 0.001);
                                    for y in ys {
                                        hline(y, window);
                                    }
                                    let mut counts = (0usize, 0usize);
                                    for (local, line) in guides.iter() {
                                        let (local, line) = (*local, line);
                                        let gi = if local {
                                            let i = counts.1;
                                            counts.1 += 1;
                                            i
                                        } else {
                                            let i = counts.0;
                                            counts.0 += 1;
                                            i
                                        };
                                        let hot = guide_hot == Some((local, gi));
                                        let base = if local {
                                            t::guide_local()
                                        } else {
                                            t::guide_line()
                                        };
                                        let color = if hot {
                                            let mut c = base;
                                            c.a = 1.0;
                                            c
                                        } else {
                                            base
                                        };
                                        let thick = if hot { 2.0 } else { 1.0 };
                                        // The knob sits on the guide's
                                        // anchor: its stored point, or the
                                        // origin axis for plain H/V lines
                                        // (UFO stores only the offset).
                                        let knob;
                                        match *line {
                                            norad::Line::Horizontal(y) => {
                                                let p = to_screen(0.0, y);
                                                window.paint_quad(gpui::fill(
                                                    Bounds::from_corners(
                                                        gpui::point(bounds.origin.x, p.y),
                                                        gpui::point(
                                                            bounds.origin.x + bounds.size.width,
                                                            p.y + px(thick),
                                                        ),
                                                    ),
                                                    color,
                                                ));
                                                knob = p;
                                            }
                                            norad::Line::Vertical(x) => {
                                                let p = to_screen(x, 0.0);
                                                window.paint_quad(gpui::fill(
                                                    Bounds::from_corners(
                                                        gpui::point(p.x, bounds.origin.y),
                                                        gpui::point(
                                                            p.x + px(thick),
                                                            bounds.origin.y + bounds.size.height,
                                                        ),
                                                    ),
                                                    color,
                                                ));
                                                knob = p;
                                            }
                                            norad::Line::Angle { x, y, degrees } => {
                                                // A segment far longer than
                                                // any canvas; the editor
                                                // clips to its bounds.
                                                let (sin, cos) = degrees.to_radians().sin_cos();
                                                const R: f64 = 1.0e5;
                                                let a = to_screen(x - R * cos, y - R * sin);
                                                let b = to_screen(x + R * cos, y + R * sin);
                                                let mut pb = PathBuilder::stroke(px(thick));
                                                pb.move_to(a);
                                                pb.line_to(b);
                                                if let Ok(path) = pb.build() {
                                                    window.paint_path(path, color);
                                                }
                                                knob = to_screen(x, y);
                                            }
                                        }
                                        // The grab knob, Glyphs-style.
                                        let r = if hot { 5.0 } else { 4.0 };
                                        let circle = {
                                            use kurbo::Shape as _;
                                            kurbo::Circle::new(
                                                (
                                                    f32::from(knob.x) as f64,
                                                    f32::from(knob.y) as f64,
                                                ),
                                                r,
                                            )
                                            .to_path(0.25)
                                        };
                                        if let Some(path) = build_fill_path(
                                            &circle,
                                            Affine::IDENTITY,
                                            gpui::point(px(0.0), px(0.0)),
                                        ) {
                                            window.paint_path(path, color);
                                        }
                                    }
                                    // Node trajectories (HOI): each
                                    // point's path across the axis as a
                                    // thin line, dots at equal axis
                                    // stops — close dots mean slow,
                                    // spread dots mean fast; brace
                                    // layers bend the line.
                                    // Knobs and the live-dragged curve
                                    // ride on top of the tracks below.
                                    if !hoi_knobs.is_empty() {
                                        use kurbo::Shape as _;
                                        if let (Some((id, q)), Some((a, b))) =
                                            (hoi_live, hoi_drag_ends)
                                        {
                                            let _ = id;
                                            let mut pb = PathBuilder::stroke(px(1.5));
                                            for step in 0..=12 {
                                                let t = step as f64 / 12.0;
                                                let p = hoi_quad_at(a, b, q, t);
                                                let sp = to_screen(p.0, p.1);
                                                if step == 0 {
                                                    pb.move_to(sp);
                                                } else {
                                                    pb.line_to(sp);
                                                }
                                            }
                                            if let Ok(line) = pb.build() {
                                                window.paint_path(line, t::accent());
                                            }
                                        }
                                        for (id, q) in &hoi_knobs {
                                            let dragging =
                                                hoi_live.is_some_and(|(live, _)| live == *id);
                                            let q = if dragging { hoi_live.unwrap().1 } else { *q };
                                            let sp = to_screen(q.0, q.1);
                                            let dot = kurbo::Circle::new(
                                                (f32::from(sp.x) as f64, f32::from(sp.y) as f64),
                                                if dragging { 4.0 } else { 2.5 },
                                            )
                                            .to_path(0.25);
                                            if let Some(path) = build_fill_path(
                                                &dot,
                                                Affine::IDENTITY,
                                                gpui::point(px(0.0), px(0.0)),
                                            ) {
                                                window.paint_path(
                                                    path,
                                                    if dragging {
                                                        t::accent()
                                                    } else {
                                                        t::text_muted()
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    if let Some(tracks) = &trajectories {
                                        use kurbo::Shape as _;
                                        // The velocity ribbon (Glyphs'
                                        // Show velocity): one block per
                                        // axis step, thickness and warmth
                                        // scaling with how far the node
                                        // travels that step — gold means
                                        // the change rushes there, ember
                                        // means it lingers.
                                        for track in tracks {
                                            let steps: Vec<f64> = track
                                                .windows(2)
                                                .map(|w| w[0].distance(w[1]))
                                                .collect();
                                            let max_step =
                                                steps.iter().fold(0.0_f64, |a, &b| a.max(b));
                                            if max_step < 1.0 {
                                                continue; // static node
                                            }
                                            const RIBBON_PX: f32 = 13.0;
                                            for (i, w) in track.windows(2).enumerate() {
                                                let speed = steps[i] / max_step;
                                                let a = to_screen(w[0].x, w[0].y);
                                                let b = to_screen(w[1].x, w[1].y);
                                                let (ax, ay) = (f32::from(a.x), f32::from(a.y));
                                                let (bx, by) = (f32::from(b.x), f32::from(b.y));
                                                let (dx_, dy_) = (bx - ax, by - ay);
                                                let len = (dx_ * dx_ + dy_ * dy_).sqrt();
                                                if len < 0.5 {
                                                    continue;
                                                }
                                                // One-sided comb, like
                                                // Glyphs': offset to the
                                                // left of travel.
                                                let (nx, ny) = (-dy_ / len, dx_ / len);
                                                let thick = RIBBON_PX * speed as f32;
                                                let mut quad = BezPath::new();
                                                quad.move_to((ax as f64, ay as f64));
                                                quad.line_to((bx as f64, by as f64));
                                                quad.line_to((
                                                    (bx + nx * thick) as f64,
                                                    (by + ny * thick) as f64,
                                                ));
                                                quad.line_to((
                                                    (ax + nx * thick) as f64,
                                                    (ay + ny * thick) as f64,
                                                ));
                                                quad.close_path();
                                                if let Some(path) = build_fill_path(
                                                    &quad,
                                                    Affine::IDENTITY,
                                                    gpui::point(px(0.0), px(0.0)),
                                                ) {
                                                    window
                                                        .paint_path(path, t::velocity_ramp(speed));
                                                }
                                            }
                                        }
                                        for track in tracks {
                                            let mut pb = PathBuilder::stroke(px(1.0));
                                            for (i, p) in track.iter().enumerate() {
                                                let sp = to_screen(p.x, p.y);
                                                if i == 0 {
                                                    pb.move_to(sp);
                                                } else {
                                                    pb.line_to(sp);
                                                }
                                            }
                                            if let Ok(line) = pb.build() {
                                                window.paint_path(line, t::trajectory_line());
                                            }
                                            let last = track.len() - 1;
                                            for (i, p) in track.iter().enumerate() {
                                                let sp = to_screen(p.x, p.y);
                                                let r = if i == 0 || i == last { 3.0 } else { 1.7 };
                                                let dot = kurbo::Circle::new(
                                                    (
                                                        f32::from(sp.x) as f64,
                                                        f32::from(sp.y) as f64,
                                                    ),
                                                    r,
                                                )
                                                .to_path(0.25);
                                                if let Some(path) = build_fill_path(
                                                    &dot,
                                                    Affine::IDENTITY,
                                                    gpui::point(px(0.0), px(0.0)),
                                                ) {
                                                    window.paint_path(path, t::trajectory_dot());
                                                }
                                            }
                                        }
                                    }
                                    for (right, x) in [(false, 0.0), (true, advance)] {
                                        let hovered = sidebearing_hover == Some(right);
                                        let a = to_screen(x, box_top);
                                        let b = to_screen(x, box_bottom);
                                        let (grow_l, grow_r) =
                                            if hovered { (1.0, 2.0) } else { (0.0, 1.0) };
                                        window.paint_quad(gpui::fill(
                                            Bounds::from_corners(
                                                gpui::point(a.x - px(grow_l), a.y),
                                                gpui::point(a.x + px(grow_r), b.y),
                                            ),
                                            if hovered {
                                                t::text_cursor()
                                            } else {
                                                t::metrics_line()
                                            },
                                        ));
                                    }
                                }

                                // Space-hold preview: the filled glyph and
                                // nothing else on top of it.
                                if preview_mode {
                                    let mut combined = outline.as_ref().clone();
                                    combined.extend(component_path.elements().iter().cloned());
                                    // The masked preview is the truth the
                                    // Bake Masks command makes permanent.
                                    if !mask_paths.is_empty() {
                                        let mut cut = BezPath::new();
                                        for m in &mask_paths {
                                            cut.extend(m.elements().iter().copied());
                                        }
                                        if let Ok(result) = linesweeper::binary_op(
                                            &combined,
                                            &cut,
                                            linesweeper::FillRule::NonZero,
                                            linesweeper::BinaryOp::Difference,
                                        ) {
                                            combined = BezPath::new();
                                            for contour in result.contours() {
                                                combined.extend(
                                                    contour.path.elements().iter().copied(),
                                                );
                                            }
                                        }
                                    }
                                    if let Some(p) = build_fill_path(&combined, transform, origin) {
                                        window.paint_path(p, t::text());
                                    }
                                }

                                // The text buffer, web-style. Quiet metric
                                // boxes first so marks and fills sit on top.
                                let zoom_now = zoom;
                                let sort_h_px =
                                    ((sort_top - sort_bottom).max(1.0) * zoom_now).max(1.0);
                                let mark = (sort_h_px * 0.05).clamp(1.5, 24.0);
                                let marks_visible = mark >= 3.0;
                                let line =
                                    |a: Point<gpui::Pixels>,
                                     b: Point<gpui::Pixels>,
                                     color: gpui::Rgba,
                                     window: &mut Window| {
                                        let mut pb = PathBuilder::stroke(px(1.0));
                                        pb.move_to(a);
                                        pb.line_to(b);
                                        if let Ok(p) = pb.build() {
                                            window.paint_path(p, color);
                                        }
                                    };
                                if !preview_mode && marks_visible {
                                    for sp in sort_paints.iter() {
                                        // Quiet full box for the sorts nobody is
                                        // editing (the active one draws its own
                                        // metrics outside text mode).
                                        if !sp.active {
                                            let color = t::metric_quiet();
                                            for ex in [sp.x, sp.x + sp.advance] {
                                                line(
                                                    to_screen(ex, sp.y + sort_bottom),
                                                    to_screen(ex, sp.y + sort_top),
                                                    color,
                                                    window,
                                                );
                                            }
                                            for my in [sort_bottom, 0.0, ascender, sort_top] {
                                                line(
                                                    to_screen(sp.x, sp.y + my),
                                                    to_screen(sp.x + sp.advance, sp.y + my),
                                                    color,
                                                    window,
                                                );
                                            }
                                        }
                                        // Corner marks: inward ticks at each
                                        // metric height on both edges, clipped
                                        // to the box. Skipped for the active
                                        // sort outside text mode (it has the
                                        // full green box instead).
                                        if sp.active && !text_mode {
                                            continue;
                                        }
                                        let color = match sp.kern {
                                            1 => t::kern_active(),
                                            2 => t::kern_previous(),
                                            _ => t::metrics_line(),
                                        };
                                        let ca = to_screen(sp.x, sp.y + sort_bottom);
                                        let cb = to_screen(sp.x + sp.advance, sp.y + sort_top);
                                        let (left, right) = (ca.x.min(cb.x), ca.x.max(cb.x));
                                        let (top_px, bottom_px) = (ca.y.min(cb.y), ca.y.max(cb.y));
                                        let mark_px = px(mark as f32);
                                        for ex in [sp.x, sp.x + sp.advance] {
                                            for my in [sort_bottom, 0.0, ascender, sort_top] {
                                                let c = to_screen(ex, sp.y + my);
                                                let x0 = (c.x - mark_px).max(left);
                                                let x1 = (c.x + mark_px).min(right);
                                                if x1 > x0 {
                                                    line(
                                                        gpui::point(x0, c.y),
                                                        gpui::point(x1, c.y),
                                                        color,
                                                        window,
                                                    );
                                                }
                                                let y0 = (c.y - mark_px).max(top_px);
                                                let y1 = (c.y + mark_px).min(bottom_px);
                                                if y1 > y0 {
                                                    line(
                                                        gpui::point(c.x, y0),
                                                        gpui::point(c.x, y1),
                                                        color,
                                                        window,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                                // Sort fills: everyone but the active sort —
                                // and the active one too while the text tool
                                // is up (points return with select). Once the
                                // design grid is up (you are drawing, not
                                // reading) the neighbours thin to a 0.34 fill
                                // plus an outline with read-only grey points,
                                // the web's zoomed-in treatment.
                                let zoomed_in = !preview_mode && zoom > 0.8;
                                // The web's point_scale curve, simplified to
                                // its zoom ramps (device scale is 1 here).
                                let point_scale = if zoom <= 0.8 {
                                    0.72 + (1.0 - 0.72) * smoothstep((zoom / 0.8).clamp(0.0, 1.0))
                                } else if zoom <= 8.0 {
                                    1.0 + 0.6 * smoothstep(((zoom - 0.8) / 7.2).clamp(0.0, 1.0))
                                } else {
                                    1.6 + 0.8 * smoothstep(((zoom - 8.0) / 20.0).clamp(0.0, 1.0))
                                };
                                for sp in sort_paints.iter() {
                                    // The active sort renders as editable
                                    // chrome except in text mode, where it is
                                    // a plain fill like its neighbors. The
                                    // preview fill already drew it.
                                    if sp.active && (!text_mode || preview_mode) {
                                        continue;
                                    }
                                    let Some(path) = sp.path.as_ref() else {
                                        continue;
                                    };
                                    let dim = zoomed_in && !sp.active;
                                    let sort_transform =
                                        transform * Affine::translate((sp.x, sp.y));
                                    if let Some(p) = build_fill_path(path, sort_transform, origin) {
                                        let mut fill = t::glyph_fill();
                                        if dim {
                                            fill.a *= 0.34;
                                        }
                                        window.paint_path(p, fill);
                                    }
                                    if !dim {
                                        continue;
                                    }
                                    // Outline + read-only points so the
                                    // neighbour reads as structure.
                                    if let Some(p) = build_path(
                                        path,
                                        sort_transform,
                                        origin,
                                        PathBuilder::stroke(px(1.0)),
                                    ) {
                                        window.paint_path(p, t::glyph_fill());
                                    }
                                    use kurbo::Shape as _;
                                    let on_r = 4.5 * point_scale * 0.85;
                                    let off_r = 4.5 * point_scale * 0.6;
                                    let screen = |pt: kurbo::Point| {
                                        let sp2 = sort_transform * pt;
                                        kurbo::Point::new(
                                            sp2.x + f64::from(f32::from(origin.x)),
                                            sp2.y + f64::from(f32::from(origin.y)),
                                        )
                                    };
                                    let mut dots = BezPath::new();
                                    let mut handles = PathBuilder::stroke(px(1.0));
                                    let mut any_handles = false;
                                    let mut current = kurbo::Point::ZERO;
                                    let mut start = kurbo::Point::ZERO;
                                    let hline2 =
                                        |a: kurbo::Point,
                                         b: kurbo::Point,
                                         pb: &mut PathBuilder,
                                         any: &mut bool| {
                                            pb.move_to(gpui::point(px(a.x as f32), px(a.y as f32)));
                                            pb.line_to(gpui::point(px(b.x as f32), px(b.y as f32)));
                                            *any = true;
                                        };
                                    for el in path.elements() {
                                        match *el {
                                            kurbo::PathEl::MoveTo(p) => {
                                                let p = screen(p);
                                                dots.extend(
                                                    kurbo::Circle::new(p, on_r).to_path(0.25),
                                                );
                                                current = p;
                                                start = p;
                                            }
                                            kurbo::PathEl::LineTo(p) => {
                                                let p = screen(p);
                                                dots.extend(
                                                    kurbo::Circle::new(p, on_r).to_path(0.25),
                                                );
                                                current = p;
                                            }
                                            kurbo::PathEl::QuadTo(c, p) => {
                                                let (c, p) = (screen(c), screen(p));
                                                dots.extend(
                                                    kurbo::Circle::new(c, off_r).to_path(0.25),
                                                );
                                                dots.extend(
                                                    kurbo::Circle::new(p, on_r).to_path(0.25),
                                                );
                                                hline2(current, c, &mut handles, &mut any_handles);
                                                hline2(c, p, &mut handles, &mut any_handles);
                                                current = p;
                                            }
                                            kurbo::PathEl::CurveTo(c1, c2, p) => {
                                                let (c1, c2, p) =
                                                    (screen(c1), screen(c2), screen(p));
                                                dots.extend(
                                                    kurbo::Circle::new(c1, off_r).to_path(0.25),
                                                );
                                                dots.extend(
                                                    kurbo::Circle::new(c2, off_r).to_path(0.25),
                                                );
                                                dots.extend(
                                                    kurbo::Circle::new(p, on_r).to_path(0.25),
                                                );
                                                hline2(current, c1, &mut handles, &mut any_handles);
                                                hline2(c2, p, &mut handles, &mut any_handles);
                                                current = p;
                                            }
                                            kurbo::PathEl::ClosePath => {
                                                current = start;
                                            }
                                        }
                                    }
                                    if any_handles && let Ok(p) = handles.build() {
                                        window.paint_path(p, t::point_readonly());
                                    }
                                    if let Some(p) = build_fill_path(
                                        &dots,
                                        Affine::IDENTITY,
                                        gpui::point(px(0.0), px(0.0)),
                                    ) {
                                        window.paint_path(p, t::point_inner());
                                    }
                                    if let Some(p) = build_path(
                                        &dots,
                                        Affine::IDENTITY,
                                        gpui::point(px(0.0), px(0.0)),
                                        PathBuilder::stroke(px(1.0)),
                                    ) {
                                        window.paint_path(p, t::point_readonly());
                                    }
                                }
                                // Caret: line plus inward triangles, sized off
                                // the sort's on-screen height like the web.
                                if let Some((cx_, cy)) = text_caret {
                                    let top = to_screen(cx_, cy + sort_top);
                                    let bottom = to_screen(cx_, cy + sort_bottom);
                                    let caret_color = t::text_cursor();
                                    window.paint_quad(gpui::fill(
                                        Bounds::from_corners(
                                            gpui::point(top.x - px(0.75), top.y),
                                            gpui::point(top.x + px(0.75), bottom.y),
                                        ),
                                        caret_color,
                                    ));
                                    let tri_scale = ((sort_h_px * 0.09).clamp(4.0, 34.0)) / 24.0;
                                    let tw = px((24.0 * tri_scale) as f32);
                                    let th = px((16.0 * tri_scale) as f32);
                                    let mut tri = PathBuilder::fill();
                                    tri.move_to(gpui::point(top.x - tw / 2.0, top.y));
                                    tri.line_to(gpui::point(top.x + tw / 2.0, top.y));
                                    tri.line_to(gpui::point(top.x, top.y + th));
                                    if let Ok(p) = tri.build() {
                                        window.paint_path(p, caret_color);
                                    }
                                    let mut tri = PathBuilder::fill();
                                    tri.move_to(gpui::point(bottom.x - tw / 2.0, bottom.y));
                                    tri.line_to(gpui::point(bottom.x + tw / 2.0, bottom.y));
                                    tri.line_to(gpui::point(bottom.x, bottom.y - th));
                                    if let Ok(p) = tri.build() {
                                        window.paint_path(p, caret_color);
                                    }
                                }

                                // Reference layers: other masters as dim strokes.
                                for path in &reference_paths {
                                    if let Some(p) = build_path(
                                        path,
                                        transform,
                                        origin,
                                        PathBuilder::stroke(px(1.0)),
                                    ) {
                                        window.paint_path(p, t::reference_layer());
                                    }
                                }

                                // Components: dim distinct fill, not editable
                                // directly (Cmd+Shift+D decomposes).
                                if !component_path.elements().is_empty()
                                    && let Some(p) =
                                        build_fill_path(&component_path, transform, origin)
                                {
                                    let color = if component_selected {
                                        t::component_selected_fill()
                                    } else {
                                        t::component_fill()
                                    };
                                    window.paint_path(p, color);
                                }
                                // Interpolated instance at the axes-bar
                                // location, as a ghost outline.
                                if let Some(ghost) = &ghost
                                    && let Some(p) = build_path(
                                        ghost,
                                        transform,
                                        origin,
                                        PathBuilder::stroke(px(1.0)),
                                    )
                                {
                                    window.paint_path(p, t::ghost());
                                }
                                // Reference glyph: a ghost fill so it never
                                // reads as the background layer's outline.
                                if let Some(path) = &reference_path
                                    && let Some(p) = build_fill_path(path, transform, origin)
                                {
                                    let mut fill = t::glyph_fill();
                                    fill.a *= 0.22;
                                    window.paint_path(p, fill);
                                }
                                // Background layer: a quiet outline behind the
                                // drawing, the way Glyphs shows a background.
                                if let Some(path) = &background_path
                                    && let Some(p) = build_path(
                                        path,
                                        transform,
                                        origin,
                                        PathBuilder::stroke(px(1.0)),
                                    )
                                {
                                    window.paint_path(p, t::metric_quiet());
                                }
                                // Per-glyph layers with the eye on: same
                                // quiet outline as the background.
                                for path in &glyph_layer_paths {
                                    if let Some(p) = build_path(
                                        path,
                                        transform,
                                        origin,
                                        PathBuilder::stroke(px(1.0)),
                                    ) {
                                        window.paint_path(p, t::metric_quiet());
                                    }
                                }
                                // The mark cloud, faint fills.
                                if !mark_cloud.is_empty() {
                                    let mut ghost = t::glyph_fill();
                                    ghost.a *= 0.10;
                                    for path in &mark_cloud {
                                        if let Some(p) = build_fill_path(path, transform, origin) {
                                            window.paint_path(p, ghost);
                                        }
                                    }
                                }
                                // Mask contours read as cuts: the local-
                                // guide accent over the normal stroke.
                                for path in &mask_paths {
                                    if let Some(p) = build_path(
                                        path,
                                        transform,
                                        origin,
                                        PathBuilder::stroke(px(2.0)),
                                    ) {
                                        window.paint_path(p, t::guide_local());
                                    }
                                }
                                // Curvature comb, behind the outline so points
                                // stay selectable over it.
                                for strip in &comb_strips {
                                    for w in strip.windows(2) {
                                        let (s0, s1) = (&w[0], &w[1]);
                                        let mut quad = BezPath::new();
                                        quad.move_to(transform * s0.on);
                                        quad.line_to(transform * s1.on);
                                        quad.line_to(transform * s1.outer);
                                        quad.line_to(transform * s0.outer);
                                        quad.close_path();
                                        let k = if comb_maxk > 1e-12 {
                                            (s0.kappa.abs() + s1.kappa.abs()) * 0.5 / comb_maxk
                                        } else {
                                            0.0
                                        };
                                        if let Some(p) =
                                            build_fill_path(&quad, Affine::IDENTITY, origin)
                                        {
                                            window.paint_path(p, t::comb_gradient(k));
                                        }
                                    }
                                }

                                // Ghost fill under the glyph being edited: the
                                // same grey the inactive sorts use at a tenth
                                // strength, so counters read as counters
                                // without competing with the outline (web
                                // ACTIVE_GLYPH_FILL_ALPHA).
                                if !preview_mode && !text_mode {
                                    let mut combined = outline.as_ref().clone();
                                    combined.extend(component_path.elements().iter().cloned());
                                    if let Some(p) = build_fill_path(&combined, transform, origin) {
                                        let mut fill = t::glyph_fill();
                                        fill.a *= 0.16;
                                        window.paint_path(p, fill);
                                    }
                                }
                                // Edit mode is a stroked outline (no fill),
                                // like the other editors.
                                if !preview_mode
                                    && !text_mode
                                    && let Some(path) = build_path(
                                        &outline,
                                        transform,
                                        origin,
                                        PathBuilder::stroke(px(1.0)),
                                    )
                                {
                                    window.paint_path(path, t::path_stroke());
                                }

                                // Handle lines: each off-curve connects to its
                                // anchoring on-curve neighbor.
                                if !preview_mode && !text_mode {
                                    let mut lines = PathBuilder::stroke(px(1.0));
                                    let mut any_line = false;
                                    for (i, p) in points.iter().enumerate() {
                                        if p.on_curve {
                                            continue;
                                        }
                                        // Neighbors within the same contour, cyclic.
                                        let contour_pts: Vec<&GlyphPoint> = points
                                            .iter()
                                            .filter(|q| q.contour == p.contour)
                                            .collect();
                                        let n = contour_pts.len();
                                        let pos = contour_pts
                                            .iter()
                                            .position(|q| q.index == p.index)
                                            .unwrap_or(0);
                                        let prev = contour_pts[(pos + n - 1) % n];
                                        let next = contour_pts[(pos + 1) % n];
                                        let anchor = if prev.on_curve {
                                            prev
                                        } else if next.on_curve {
                                            next
                                        } else {
                                            continue;
                                        };
                                        lines.move_to(to_screen(p.x, p.y));
                                        lines.line_to(to_screen(anchor.x, anchor.y));
                                        any_line = true;
                                        let _ = i;
                                    }
                                    if any_line && let Ok(path) = lines.build() {
                                        window.paint_path(path, t::handle_line());
                                    }
                                }

                                // Points: smooth = blue circle, corner = green
                                // square, off-curve = purple circle, selection
                                // in yellow/orange — the shared palette.
                                let circle =
                                    |window: &mut Window,
                                     center: Point<gpui::Pixels>,
                                     r: f32,
                                     color: gpui::Rgba| {
                                        use kurbo::Shape;
                                        let cx_: f32 = center.x.into();
                                        let cy_: f32 = center.y.into();
                                        let shape =
                                            kurbo::Circle::new((cx_ as f64, cy_ as f64), r as f64)
                                                .to_path(0.25);
                                        if let Some(p) = build_fill_path(
                                            &shape,
                                            Affine::IDENTITY,
                                            gpui::point(px(0.0), px(0.0)),
                                        ) {
                                            window.paint_path(p, color);
                                        }
                                    };
                                // A point is a dark window with a coloured
                                // ring, the web's recipe: a halo casing so
                                // it keeps an edge over the outline and the
                                // comb, an interior fill that masks what
                                // runs underneath, then a constant-width
                                // ring on top. Selected points fill yellow
                                // and ring in the selection colour.
                                let ps = point_scale as f32;
                                let ring_w = (1.5 * ps).max(1.0);
                                let halo_w = ring_w + 2.0;
                                let shape = |center: Point<gpui::Pixels>,
                                             r: f32,
                                             square: bool|
                                 -> kurbo::BezPath {
                                    use kurbo::Shape as _;
                                    let (cx_, cy_) =
                                        (f32::from(center.x) as f64, f32::from(center.y) as f64);
                                    if square {
                                        kurbo::Rect::new(
                                            cx_ - r as f64,
                                            cy_ - r as f64,
                                            cx_ + r as f64,
                                            cy_ + r as f64,
                                        )
                                        .to_path(0.1)
                                    } else {
                                        kurbo::Circle::new((cx_, cy_), r as f64).to_path(0.15)
                                    }
                                };
                                let zero = gpui::point(px(0.0), px(0.0));
                                // Colours are the batch key: an Rgba is not
                                // hashable, so its bytes stand in.
                                let color_key = |c: gpui::Rgba| -> u32 {
                                    u32::from_be_bytes([
                                        (c.r * 255.0) as u8,
                                        (c.g * 255.0) as u8,
                                        (c.b * 255.0) as u8,
                                        (c.a * 255.0) as u8,
                                    ])
                                };
                                let mut halo_batch: Vec<BezPath> = Vec::new();
                                let mut fill_batch: std::collections::BTreeMap<
                                    u32,
                                    (gpui::Rgba, Vec<BezPath>),
                                > = std::collections::BTreeMap::new();
                                let mut ring_batch: std::collections::BTreeMap<
                                    u32,
                                    (gpui::Rgba, Vec<BezPath>),
                                > = std::collections::BTreeMap::new();
                                #[allow(clippy::type_complexity)]
                            let mut chord_batch: std::collections::BTreeMap<
                                u32,
                                (gpui::Rgba, Vec<(f32, BezPath)>),
                            > = std::collections::BTreeMap::new();
                                for p in points.iter() {
                                    if preview_mode || text_mode {
                                        break;
                                    }
                                    let center = to_screen(p.x, p.y);
                                    let is_selected =
                                        selected_points.contains(&(p.contour, p.index));
                                    let is_locked = locked_points.contains(&(p.contour, p.index));
                                    let (ring, inner) = if is_locked {
                                        // Locked nodes read as inert.
                                        (t::point_readonly(), t::point_readonly())
                                    } else if is_selected {
                                        (t::point_selected_ring(), t::point_selected())
                                    } else if p.hyper {
                                        (t::point_hyper_outer(), t::point_inner())
                                    } else if !p.on_curve {
                                        (t::point_offcurve_outer(), t::point_inner())
                                    } else if p.smooth {
                                        (t::point_smooth_outer(), t::point_inner())
                                    } else {
                                        (t::point_corner_outer(), t::point_inner())
                                    };
                                    let is_square = p.on_curve && !p.smooth && !p.hyper;
                                    let r = if p.hyper && p.on_curve {
                                        if is_selected { 5.0 } else { 4.0 }
                                    } else if is_square {
                                        if is_selected { 4.5 } else { 3.5 }
                                    } else if is_selected {
                                        5.5
                                    } else {
                                        4.5
                                    } * ps;
                                    let path = shape(center, r, is_square);
                                    halo_batch.push(path.clone());
                                    fill_batch
                                        .entry(color_key(inner))
                                        .or_insert_with(|| (inner, Vec::new()))
                                        .1
                                        .push(path.clone());
                                    // The point is a window onto the design
                                    // grid: the gridlines that cross it are
                                    // redrawn inside, tinted with the
                                    // point's own colour, so you can read
                                    // where it sits (web draws this by
                                    // clipping the grid to the point; gpui
                                    // masks rectangles only, so the chords
                                    // are solved instead — exact, and it
                                    // costs a few lines per point).
                                    if grid_mid_alpha > 0.0 && !preview_mode && !text_mode {
                                        let (cx_, cy_) = (
                                            f32::from(center.x) as f64,
                                            f32::from(center.y) as f64,
                                        );
                                        let r = r as f64;
                                        let inv = transform.inverse();
                                        for (spacing, alpha, wide) in [
                                            (8.0_f64, grid_mid_alpha, 1.0_f32),
                                            (2.0, grid_close_alpha, 0.7),
                                        ] {
                                            if alpha <= 0.0 {
                                                continue;
                                            }
                                            let mut tint = ring;
                                            tint.a = alpha as f32;
                                            let mut lines = BezPath::new();
                                            // Vertical gridlines: the chord
                                            // is the circle's half-height at
                                            // that offset (the full radius
                                            // for a square point).
                                            let a = (inv * kurbo::Point::new(cx_ - r, cy_)).x;
                                            let b = (inv * kurbo::Point::new(cx_ + r, cy_)).x;
                                            let (lo, hi) = (a.min(b), a.max(b));
                                            for k in (lo / spacing).ceil() as i64
                                                ..=(hi / spacing).floor() as i64
                                            {
                                                let sx = (transform
                                                    * kurbo::Point::new(k as f64 * spacing, 0.0))
                                                .x;
                                                let d = sx - cx_;
                                                let half = if is_square {
                                                    r
                                                } else {
                                                    (r * r - d * d).max(0.0).sqrt()
                                                };
                                                if half <= 0.2 {
                                                    continue;
                                                }
                                                lines.move_to((sx, cy_ - half));
                                                lines.line_to((sx, cy_ + half));
                                            }
                                            let a = (inv * kurbo::Point::new(cx_, cy_ - r)).y;
                                            let b = (inv * kurbo::Point::new(cx_, cy_ + r)).y;
                                            let (lo, hi) = (a.min(b), a.max(b));
                                            for k in (lo / spacing).ceil() as i64
                                                ..=(hi / spacing).floor() as i64
                                            {
                                                let sy = (transform
                                                    * kurbo::Point::new(0.0, k as f64 * spacing))
                                                .y;
                                                let d = sy - cy_;
                                                let half = if is_square {
                                                    r
                                                } else {
                                                    (r * r - d * d).max(0.0).sqrt()
                                                };
                                                if half <= 0.2 {
                                                    continue;
                                                }
                                                lines.move_to((cx_ - half, sy));
                                                lines.line_to((cx_ + half, sy));
                                            }
                                            if !lines.is_empty() {
                                                let entry = chord_batch
                                                    .entry(color_key(tint))
                                                    .or_insert_with(|| (tint, Vec::new()));
                                                match entry.1.iter_mut().find(|(w, _)| *w == wide) {
                                                    Some((_, acc)) => acc.extend(lines.iter()),
                                                    None => entry.1.push((wide, lines)),
                                                }
                                            }
                                        }
                                    }
                                    ring_batch
                                        .entry(color_key(ring))
                                        .or_insert_with(|| (ring, Vec::new()))
                                        .1
                                        .push(path);
                                }
                                // Three path draws for every point on the
                                // glyph, plus the gridlines, collapse into
                                // one per colour.
                                paint_batched(window, zero, t::halo(), &halo_batch, Some(halo_w));
                                for (color, paths) in fill_batch.values() {
                                    paint_batched(window, zero, *color, paths, None);
                                }
                                for (color, path) in chord_batch.values() {
                                    for (width, path) in path {
                                        if let Some(p) = build_path(
                                            path,
                                            Affine::IDENTITY,
                                            zero,
                                            PathBuilder::stroke(px(*width)),
                                        ) {
                                            window.paint_path(p, *color);
                                        }
                                    }
                                }
                                for (color, paths) in ring_batch.values() {
                                    paint_batched(window, zero, *color, paths, Some(ring_w));
                                }
                                // Start-of-contour arrow: which point a closed
                                // contour begins at, and which way it runs
                                // (web draw_start_arrow).
                                if !preview_mode && !text_mode {
                                    for start in start_markers.iter() {
                                        let (from, to, selected) = *start;
                                        let a = to_screen(from.0, from.1);
                                        let b = to_screen(to.0, to.1);
                                        let size = (if selected { 6.5 } else { 5.5 }) * ps;
                                        let dir = (f32::from(b.x - a.x), f32::from(b.y - a.y));
                                        let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
                                        if len < 0.001 {
                                            continue;
                                        }
                                        let f = (dir.0 / len, dir.1 / len);
                                        let perp = (-f.1, f.0);
                                        let cx_ = f32::from(a.x) + perp.0 * 8.0 * ps;
                                        let cy_ = f32::from(a.y) + perp.1 * 8.0 * ps;
                                        let tip = (cx_ + f.0 * size, cy_ + f.1 * size);
                                        let base = (cx_ - f.0 * size * 0.5, cy_ - f.1 * size * 0.5);
                                        let left = (
                                            base.0 + perp.0 * size * 0.5,
                                            base.1 + perp.1 * size * 0.5,
                                        );
                                        let right = (
                                            base.0 - perp.0 * size * 0.5,
                                            base.1 - perp.1 * size * 0.5,
                                        );
                                        let mut pb = PathBuilder::fill();
                                        pb.move_to(gpui::point(px(tip.0), px(tip.1)));
                                        pb.line_to(gpui::point(px(left.0), px(left.1)));
                                        pb.line_to(gpui::point(px(right.0), px(right.1)));
                                        pb.close();
                                        if let Ok(path) = pb.build() {
                                            window.paint_path(
                                                path,
                                                if selected {
                                                    t::point_selected()
                                                } else {
                                                    t::point_smooth_outer()
                                                },
                                            );
                                        }
                                    }
                                }
                                // Anchors: diamonds (rotated squares drawn as
                                // two overlapping quads approximate; use a
                                // filled path).
                                // Anchors are diamonds built like points: a
                                // dark window with a coloured ring, sized
                                // off the smooth-point radius and widened a
                                // little so a rotated square reads as the
                                // same size (web ANCHOR_DIAMOND_SCALE).
                                let mut anchor_halo: Vec<BezPath> = Vec::new();
                                let mut anchor_fill: std::collections::BTreeMap<
                                    u32,
                                    (gpui::Rgba, Vec<BezPath>),
                                > = std::collections::BTreeMap::new();
                                let mut anchor_ring: std::collections::BTreeMap<
                                    u32,
                                    (gpui::Rgba, Vec<BezPath>),
                                > = std::collections::BTreeMap::new();
                                for (ai, (_, ax, ay)) in anchors.iter().enumerate() {
                                    if preview_mode || text_mode {
                                        break;
                                    }
                                    let center = to_screen(*ax, *ay);
                                    let is_selected = selected_anchors.contains(&ai);
                                    let r = (if is_selected { 5.5 } else { 4.5 }) * ps * 1.35;
                                    let (cx_, cy_) =
                                        (f32::from(center.x) as f64, f32::from(center.y) as f64);
                                    let r = r as f64;
                                    let mut diamond = BezPath::new();
                                    diamond.move_to((cx_, cy_ - r));
                                    diamond.line_to((cx_ + r, cy_));
                                    diamond.line_to((cx_, cy_ + r));
                                    diamond.line_to((cx_ - r, cy_));
                                    diamond.close_path();
                                    let (ring, inner) = if is_selected {
                                        (t::point_selected_ring(), t::point_selected())
                                    } else {
                                        (t::anchor(), t::point_inner())
                                    };
                                    anchor_halo.push(diamond.clone());
                                    anchor_fill
                                        .entry(color_key(inner))
                                        .or_insert_with(|| (inner, Vec::new()))
                                        .1
                                        .push(diamond.clone());
                                    anchor_ring
                                        .entry(color_key(ring))
                                        .or_insert_with(|| (ring, Vec::new()))
                                        .1
                                        .push(diamond);
                                }
                                paint_batched(window, zero, t::halo(), &anchor_halo, Some(halo_w));
                                for (color, paths) in anchor_fill.values() {
                                    paint_batched(window, zero, *color, paths, None);
                                }
                                for (color, paths) in anchor_ring.values() {
                                    paint_batched(window, zero, *color, paths, Some(ring_w));
                                }

                                // Shapes-tool live preview.
                                if let Some((a, b, ellipse)) = shape_preview {
                                    use kurbo::Shape as _;
                                    let rect = kurbo::Rect::from_points(
                                        kurbo::Point::new(a.0, a.1),
                                        kurbo::Point::new(b.0, b.1),
                                    );
                                    let shape: BezPath = if ellipse {
                                        kurbo::Ellipse::from_rect(rect).to_path(0.1)
                                    } else {
                                        rect.to_path(0.1)
                                    };
                                    if let Some(p) = build_path(
                                        &shape,
                                        transform,
                                        origin,
                                        PathBuilder::stroke(px(1.0)),
                                    ) {
                                        window.paint_path(p, t::accent());
                                    }
                                }
                                // Measure-tool line.
                                if let Some(seg) = hover_seg {
                                    let mut pb = PathBuilder::stroke(px(3.0));
                                    match seg {
                                        kurbo::PathSeg::Line(l) => {
                                            pb.move_to(to_screen(l.p0.x, l.p0.y));
                                            pb.line_to(to_screen(l.p1.x, l.p1.y));
                                        }
                                        kurbo::PathSeg::Quad(q) => {
                                            pb.move_to(to_screen(q.p0.x, q.p0.y));
                                            pb.curve_to(
                                                to_screen(q.p2.x, q.p2.y),
                                                to_screen(q.p1.x, q.p1.y),
                                            );
                                        }
                                        kurbo::PathSeg::Cubic(c) => {
                                            pb.move_to(to_screen(c.p0.x, c.p0.y));
                                            pb.cubic_bezier_to(
                                                to_screen(c.p3.x, c.p3.y),
                                                to_screen(c.p1.x, c.p1.y),
                                                to_screen(c.p2.x, c.p2.y),
                                            );
                                        }
                                    }
                                    if let Ok(p) = pb.build() {
                                        window.paint_path(p, t::accent());
                                    }
                                }
                                if let Some(((lx, ly), (cx3, cy3), close)) = pen_preview {
                                    let mut pb = PathBuilder::stroke(px(1.0));
                                    pb.move_to(to_screen(lx, ly));
                                    pb.line_to(to_screen(cx3, cy3));
                                    if let Ok(p) = pb.build() {
                                        window.paint_path(p, t::accent());
                                    }
                                    if let Some((sx2, sy2)) = close {
                                        circle(window, to_screen(sx2, sy2), 6.0, t::accent());
                                    }
                                }
                                if let Some(((sx, sy), (cx2, cy2), hits)) = &knife_line {
                                    let a = to_screen(*sx, *sy);
                                    let b = to_screen(*cx2, *cy2);
                                    let mut line = PathBuilder::stroke(px(1.0));
                                    line.move_to(a);
                                    line.line_to(b);
                                    if let Ok(p) = line.build() {
                                        window.paint_path(p, t::anchor());
                                    }
                                    for hit in hits {
                                        let c = to_screen(hit.x, hit.y);
                                        circle(window, c, 3.5, t::anchor());
                                    }
                                }
                                if let Some((a, b)) = measure_line {
                                    let mut pb = PathBuilder::stroke(px(1.0));
                                    let pa = to_screen(a.0, a.1);
                                    let pbp = to_screen(b.0, b.1);
                                    pb.move_to(pa);
                                    pb.line_to(pbp);
                                    if let Ok(p) = pb.build() {
                                        window.paint_path(p, t::accent());
                                    }
                                }
                                // Measure-tool HUD (web draw_measurements):
                                // popcount-colored outline, dimension lines
                                // with outward arrowheads, and labels that
                                // dodge each other. Fades in with zoom.
                                if let Some((strokes, measurements, sb)) = &measure_hud {
                                    use runebender_core::measure::{self, MeasureKind};
                                    let t32 = (((zoom - 0.30) / 0.40).clamp(0.0, 1.0)) as f32;
                                    if t32 > 0.0 {
                                        let fade = |mut c: gpui::Rgba, mul: f32| {
                                            c.a *= t32 * mul;
                                            c
                                        };
                                        for cs in strokes {
                                            let width = if cs.wide { 1.5 } else { 1.0 };
                                            if let Some(p) = build_path(
                                                &cs.path,
                                                transform,
                                                origin,
                                                PathBuilder::stroke(px(width)),
                                            ) {
                                                window.paint_path(
                                                    p,
                                                    fade(t::popcount_tier(cs.popcount), 1.0),
                                                );
                                            }
                                        }
                                        let gp = |p: kurbo::Point| {
                                            gpui::point(
                                                origin.x + px(p.x as f32),
                                                origin.y + px(p.y as f32),
                                            )
                                        };
                                        // A span's dimension line: a shaft that
                                        // stops short of both endpoints with an
                                        // outward arrowhead at each end.
                                        let dim_line = |window: &mut gpui::Window,
                                                    a: kurbo::Point,
                                                    b: kurbo::Point,
                                                    color: gpui::Rgba| {
                                        let (dx, dy) = (b.x - a.x, b.y - a.y);
                                        let len = dx.hypot(dy);
                                        if len < 1e-3 {
                                            return;
                                        }
                                        let (ux, uy) = (dx / len, dy / len);
                                        let (nx, ny) = (-uy, ux);
                                        let (end_gap, head, wing) = (3.0, 7.0, 4.0);
                                        let a2 = kurbo::Point::new(
                                            a.x + ux * end_gap,
                                            a.y + uy * end_gap,
                                        );
                                        let b2 = kurbo::Point::new(
                                            b.x - ux * end_gap,
                                            b.y - uy * end_gap,
                                        );
                                        let mut pb = PathBuilder::stroke(px(1.25));
                                        pb.move_to(gp(a2));
                                        pb.line_to(gp(b2));
                                        for (p0, sx) in [(a2, 1.0), (b2, -1.0)] {
                                            for side in [1.0, -1.0] {
                                                pb.move_to(gp(p0));
                                                pb.line_to(gp(kurbo::Point::new(
                                                    p0.x + sx * ux * head + side * nx * wing,
                                                    p0.y + sx * uy * head + side * ny * wing,
                                                )));
                                            }
                                        }
                                        if let Ok(p) = pb.build() {
                                            window.paint_path(p, color);
                                        }
                                    };
                                        // Place a label just off its line, then
                                        // step it outward (and to the other
                                        // side) until it clears every label
                                        // already placed this frame.
                                        let label_px = px(13.0);
                                        let line_h = px(15.0);
                                        let label_font = window.text_style().font();
                                        let mut placed: Vec<kurbo::Rect> = Vec::new();
                                        let draw_label =
                                        |window: &mut gpui::Window,
                                         cx: &mut gpui::App,
                                         a: kurbo::Point,
                                         b: kurbo::Point,
                                         text: String,
                                         color: gpui::Rgba,
                                         placed: &mut Vec<kurbo::Rect>| {
                                            let label_text =
                                                gpui::SharedString::from(text);
                                            let run = gpui::TextRun {
                                                len: label_text.len(),
                                                font: label_font.clone(),
                                                color: color.into(),
                                                background_color: None,
                                                underline: None,
                                                strikethrough: None,
                                            };
                                            let line = window.text_system().shape_line(
                                                label_text.clone(),
                                                label_px,
                                                std::slice::from_ref(&run),
                                                None,
                                            );
                                            let w = f32::from(line.width) as f64;
                                            let h = f32::from(line_h) as f64;
                                            let (dx, dy) = (b.x - a.x, b.y - a.y);
                                            let len = dx.hypot(dy).max(1e-6);
                                            let (mut nx, mut ny) = (-dy / len, dx / len);
                                            let horizontalish = dx.abs() >= dy.abs();
                                            if (horizontalish && ny > 0.0)
                                                || (!horizontalish && nx < 0.0)
                                            {
                                                nx = -nx;
                                                ny = -ny;
                                            }
                                            let mid = a.midpoint(b);
                                            let (base, step, pad) = (6.0, h + 4.0, 2.0);
                                            let top_left = |dirx: f64, diry: f64, dist: f64| {
                                                let cx0 = mid.x + dirx * dist;
                                                let cy0 = mid.y + diry * dist;
                                                let x = if dirx > 0.3 {
                                                    cx0
                                                } else if dirx < -0.3 {
                                                    cx0 - w
                                                } else {
                                                    cx0 - w / 2.0
                                                };
                                                let y = if diry > 0.3 {
                                                    cy0
                                                } else if diry < -0.3 {
                                                    cy0 - h
                                                } else {
                                                    cy0 - h / 2.0
                                                };
                                                kurbo::Point::new(x, y)
                                            };
                                            let mut chosen = top_left(nx, ny, base);
                                            'search: for &sign in &[1.0_f64, -1.0] {
                                                let (dirx, diry) = (nx * sign, ny * sign);
                                                for k in 0..6 {
                                                    let cand = top_left(
                                                        dirx,
                                                        diry,
                                                        base + k as f64 * step,
                                                    );
                                                    let rect = kurbo::Rect::new(
                                                        cand.x - pad,
                                                        cand.y - pad,
                                                        cand.x + w + pad,
                                                        cand.y + h + pad,
                                                    );
                                                    let clear = !placed.iter().any(|r| {
                                                        r.x0 < rect.x1
                                                            && rect.x0 < r.x1
                                                            && r.y0 < rect.y1
                                                            && rect.y0 < r.y1
                                                    });
                                                    if clear {
                                                        chosen = cand;
                                                        break 'search;
                                                    }
                                                }
                                            }
                                            placed.push(kurbo::Rect::new(
                                                chosen.x,
                                                chosen.y,
                                                chosen.x + w,
                                                chosen.y + h,
                                            ));
                                            // A casing around the
                                            // numerals, not a filled
                                            // box: the web strokes each
                                            // glyph in the halo colour
                                            // before filling it. gpui
                                            // has no stroked text, so
                                            // the line is painted eight
                                            // times around the centre
                                            // instead, which reads the
                                            // same and keeps the canvas
                                            // visible behind the label.
                                            let mut halo_color = t::halo();
                                            halo_color.a *= t32;
                                            let halo_run = gpui::TextRun {
                                                len: run.len,
                                                font: label_font.clone(),
                                                color: halo_color.into(),
                                                background_color: None,
                                                underline: None,
                                                strikethrough: None,
                                            };
                                            let halo_line =
                                                window.text_system().shape_line(
                                                    label_text.clone(),
                                                    label_px,
                                                    std::slice::from_ref(&halo_run),
                                                    None,
                                                );
                                            for (ox, oy) in [
                                                (-1.0, 0.0),
                                                (1.0, 0.0),
                                                (0.0, -1.0),
                                                (0.0, 1.0),
                                                (-1.0, -1.0),
                                                (1.0, -1.0),
                                                (-1.0, 1.0),
                                                (1.0, 1.0),
                                            ] {
                                                let _ = halo_line.paint(
                                                    gp(kurbo::Point::new(
                                                        chosen.x + ox,
                                                        chosen.y + oy,
                                                    )),
                                                    line_h,
                                                    gpui::TextAlign::Left,
                                                    None,
                                                    window,
                                                    cx,
                                                );
                                            }
                                            let _ = line.paint(
                                                gp(chosen),
                                                line_h,
                                                gpui::TextAlign::Left,
                                                None,
                                                window,
                                                cx,
                                            );
                                        };
                                        if let Some(sb) = sb {
                                            for (is_left, x, y, val) in [
                                                (true, sb.min_x, sb.y_left, sb.lsb),
                                                (false, sb.max_x, sb.y_right, sb.rsb),
                                            ] {
                                                let color = fade(
                                                    t::popcount_tier(measure::popcount(val)),
                                                    0.9,
                                                );
                                                let margin_x =
                                                    if is_left { 0.0 } else { sb.advance };
                                                let a = transform * kurbo::Point::new(margin_x, y);
                                                let b = transform * kurbo::Point::new(x, y);
                                                dim_line(window, a, b, color);
                                                draw_label(
                                                    window,
                                                    cx,
                                                    a,
                                                    b,
                                                    measure_opts.label(val),
                                                    color,
                                                    &mut placed,
                                                );
                                            }
                                        }
                                        for m in measurements {
                                            let show = match m.kind {
                                                MeasureKind::Handle => measure_opts.handles,
                                                MeasureKind::Segment => measure_opts.segments,
                                                MeasureKind::Horizontal | MeasureKind::Vertical => {
                                                    measure_opts.spans
                                                }
                                            };
                                            if !show {
                                                continue;
                                            }
                                            let a = transform * m.a;
                                            let b = transform * m.b;
                                            let color = fade(
                                                t::popcount_tier(measure::popcount(m.length)),
                                                1.0,
                                            );
                                            if matches!(
                                                m.kind,
                                                MeasureKind::Horizontal | MeasureKind::Vertical
                                            ) {
                                                dim_line(window, a, b, color);
                                            }
                                            draw_label(
                                                window,
                                                cx,
                                                a,
                                                b,
                                                measure_opts.label(m.length),
                                                color,
                                                &mut placed,
                                            );
                                        }
                                        // Segment sizes: each curve's own
                                        // box, labelled at its centre, so
                                        // the whole glyph can be read at
                                        // once instead of one selection at
                                        // a time.
                                        for b in segment_boxes.iter() {
                                            let c0 = transform * kurbo::Point::new(b.x0, b.y0);
                                            let c1 = transform * kurbo::Point::new(b.x1, b.y1);
                                            let rect = kurbo::Rect::from_points(c0, c1);
                                            let mut frame = PathBuilder::stroke(px(1.0));
                                            let corners = [
                                                (rect.x0, rect.y0),
                                                (rect.x1, rect.y0),
                                                (rect.x1, rect.y1),
                                                (rect.x0, rect.y1),
                                            ];
                                            frame.move_to(gp(kurbo::Point::new(
                                                corners[0].0,
                                                corners[0].1,
                                            )));
                                            for (x, y) in corners.iter().skip(1) {
                                                frame.line_to(gp(kurbo::Point::new(*x, *y)));
                                            }
                                            frame.line_to(gp(kurbo::Point::new(
                                                corners[0].0,
                                                corners[0].1,
                                            )));
                                            let color = fade(t::metric_quiet(), 1.0);
                                            if let Ok(p) = frame.build() {
                                                window.paint_path(p, color);
                                            }
                                            let text =
                                                format!("{:.0}×{:.0}", b.width(), b.height());
                                            let mid_left =
                                                kurbo::Point::new(rect.x0, rect.center().y);
                                            let mid_right =
                                                kurbo::Point::new(rect.x1, rect.center().y);
                                            draw_label(
                                                window,
                                                cx,
                                                mid_left,
                                                mid_right,
                                                text,
                                                fade(t::text(), 1.0),
                                                &mut placed,
                                            );
                                        }
                                    }
                                }
                                // Continuity rings around on-curve nodes.
                                if !continuity_rings.is_empty() {
                                    use kurbo::Shape as _;
                                    let r = (4.5 * 1.9) as f64;
                                    for (at, color) in &continuity_rings {
                                        let c = transform * *at;
                                        let circle = kurbo::Circle::new(c, r).to_path(0.25);
                                        if let Some(p) = build_path(
                                            &circle,
                                            Affine::IDENTITY,
                                            origin,
                                            PathBuilder::stroke(px(1.5)),
                                        ) {
                                            window.paint_path(p, *color);
                                        }
                                    }
                                }

                                // Annotations: red working marks over
                                // everything (arrows point at the spot,
                                // circles ring it, notes label it).
                                if !annotations.is_empty() {
                                    use kurbo::Shape as _;
                                    let color = t::annotation();
                                    for note in &annotations {
                                        let p = to_screen(note.x, note.y);
                                        let (px_, py_) =
                                            (f32::from(p.x) as f64, f32::from(p.y) as f64);
                                        match note.kind.as_str() {
                                            "circle" => {
                                                let ring = kurbo::Circle::new((px_, py_), 12.0)
                                                    .to_path(0.25);
                                                if let Some(path) = build_path(
                                                    &ring,
                                                    Affine::IDENTITY,
                                                    gpui::point(px(0.0), px(0.0)),
                                                    PathBuilder::stroke(px(2.0)),
                                                ) {
                                                    window.paint_path(path, color);
                                                }
                                            }
                                            "note" => {
                                                let dot = kurbo::Circle::new((px_, py_), 3.0)
                                                    .to_path(0.25);
                                                if let Some(path) = build_fill_path(
                                                    &dot,
                                                    Affine::IDENTITY,
                                                    gpui::point(px(0.0), px(0.0)),
                                                ) {
                                                    window.paint_path(path, color);
                                                }
                                                let text =
                                                    gpui::SharedString::from(note.text.clone());
                                                let run = gpui::TextRun {
                                                    len: text.len(),
                                                    font: window.text_style().font(),
                                                    color: color.into(),
                                                    background_color: None,
                                                    underline: None,
                                                    strikethrough: None,
                                                };
                                                let line = window.text_system().shape_line(
                                                    text,
                                                    px(12.0),
                                                    std::slice::from_ref(&run),
                                                    None,
                                                );
                                                let _ = line.paint(
                                                    gpui::point(p.x + px(8.0), p.y - px(7.0)),
                                                    px(14.0),
                                                    gpui::TextAlign::Left,
                                                    None,
                                                    window,
                                                    cx,
                                                );
                                            }
                                            _ => {
                                                // Arrow from lower-right,
                                                // tip on the point.
                                                let mut arrow = BezPath::new();
                                                arrow.move_to((px_, py_));
                                                arrow.line_to((px_ + 12.0, py_ + 4.0));
                                                arrow.line_to((px_ + 8.0, py_ + 8.0));
                                                arrow.line_to((px_ + 20.0, py_ + 20.0));
                                                arrow.line_to((px_ + 8.0 + 4.0, py_ + 8.0 + 8.0));
                                                arrow.line_to((px_ + 4.0, py_ + 12.0));
                                                arrow.close_path();
                                                if let Some(path) = build_fill_path(
                                                    &arrow,
                                                    Affine::IDENTITY,
                                                    gpui::point(px(0.0), px(0.0)),
                                                ) {
                                                    window.paint_path(path, color);
                                                }
                                            }
                                        }
                                    }
                                }
                                // Free-transform box: the selection's
                                // bounds, corner and edge handles, all
                                // constant screen size (Glyphs 4's
                                // on-canvas rotate and scale).
                                if let Some(bbox) = transform_box {
                                    let pa = to_screen(bbox.x0, bbox.y0);
                                    let pb = to_screen(bbox.x1, bbox.y1);
                                    let rect = Bounds::from_corners(
                                        gpui::point(pa.x.min(pb.x), pa.y.min(pb.y)),
                                        gpui::point(pa.x.max(pb.x), pa.y.max(pb.y)),
                                    );
                                    window.paint_quad(gpui::outline(
                                        rect,
                                        t::marquee_stroke(),
                                        gpui::BorderStyle::Solid,
                                    ));
                                    let (cx_, cy_) = (bbox.center().x, bbox.center().y);
                                    const HANDLE: f32 = 6.0;
                                    for (hx, hy) in [
                                        (bbox.x0, bbox.y0),
                                        (bbox.x1, bbox.y0),
                                        (bbox.x0, bbox.y1),
                                        (bbox.x1, bbox.y1),
                                        (cx_, bbox.y0),
                                        (cx_, bbox.y1),
                                        (bbox.x0, cy_),
                                        (bbox.x1, cy_),
                                    ] {
                                        let p = to_screen(hx, hy);
                                        let half = px(HANDLE / 2.0);
                                        let handle = Bounds::from_corners(
                                            gpui::point(p.x - half, p.y - half),
                                            gpui::point(p.x + half, p.y + half),
                                        );
                                        window.paint_quad(gpui::fill(handle, t::panel_bg()));
                                        window.paint_quad(gpui::outline(
                                            handle,
                                            t::marquee_stroke(),
                                            gpui::BorderStyle::Solid,
                                        ));
                                    }
                                }
                                // Marquee rectangle.
                                if let Some((a, b)) = marquee {
                                    let pa = to_screen(a.0, a.1);
                                    let pb = to_screen(b.0, b.1);
                                    let rect = Bounds::from_corners(
                                        gpui::point(pa.x.min(pb.x), pa.y.min(pb.y)),
                                        gpui::point(pa.x.max(pb.x), pa.y.max(pb.y)),
                                    );
                                    window.paint_quad(gpui::fill(rect, t::marquee_fill()));
                                    window.paint_quad(gpui::outline(
                                        rect,
                                        t::marquee_stroke(),
                                        gpui::BorderStyle::Solid,
                                    ));
                                }
                                let _ = (zoom, &component_names);
                            },
                        );
                    },
                )
                .size_full(),
            )
            .child(self.editor_info_panel(index, cx))
    }
}
