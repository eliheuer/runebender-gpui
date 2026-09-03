// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The nodes canvas: the open `.nodes.json` as boxes and wires.
//!
//! Built on the glyph editor's parts. Core's `ViewPort` maps canvas
//! units to pixels and does the zoom about the cursor; the drag state
//! machine and the hit-testing live in `edit/nodes.rs` beside the
//! panel's rows; and everything here is one `canvas` element painted
//! with `PathBuilder`, the way the editing view paints an outline.
//! Nothing is a `div`, so a box and its wires scale together and one
//! paint pass draws the lot.

use crate::Workspace;
use crate::edit::nodes::{NodeBox, NodesView, RowState, to_screen};
use crate::view::controls as c;
use crate::view::render::px32;
use crate::view::theme as t;
use gpui::App;
use gpui::Bounds;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::MouseButton;
use gpui::ParentElement;
use gpui::PathBuilder;
use gpui::Point;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::Window;
use gpui::canvas;
use gpui::div;
use gpui::px;
use runebender_core::document::nodes_run::Status;
use runebender_core::ui::editing::viewport::ViewPort;

/// What one paint of the canvas needs, gathered while `self` is
/// borrowed and handed to the paint closure.
struct NodesScene {
    /// Every node, laid out.
    boxes: Vec<NodeBox>,
    /// `(from box index, output index, to box index, input index)`.
    wires: Vec<(usize, usize, usize, usize)>,
    /// A wire being dragged: from a port, to a window point.
    pending: Option<(Point<gpui::Pixels>, Point<gpui::Pixels>)>,
    /// The selected node.
    selected: Option<u32>,
    /// Each node's run state, for the header mark and the note.
    rows: std::collections::BTreeMap<u32, RowState>,
    /// Canvas units to pixels.
    viewport: ViewPort,
    /// Why the file will not run, if it will not.
    problems: Vec<SharedString>,
    /// Where the paint records the canvas bounds.
    bounds_slot: std::sync::Arc<std::sync::Mutex<Bounds<gpui::Pixels>>>,
}

impl Workspace {
    /// The strip above the canvas: the file, Save, Run, and one
    /// button per node type to add.
    pub(crate) fn nodes_strip(&self, cx: &mut Context<'_, Self>) -> gpui::Div {
        let Some(state) = self.models.graph.as_ref() else {
            return c::row();
        };
        let label = crate::edit::nodes::file_label(&state.path);
        let running = state.running;
        let mut adds = c::row().flex_wrap();
        for ty in state.registry.types.iter().filter(|t| t.implemented) {
            let name = ty.name.clone();
            adds = adds.child(
                c::button(
                    SharedString::from(format!("nodes-add-{}", ty.name)),
                    ty.title.clone(),
                )
                .flex_none()
                .px_2()
                .w_auto()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.nodes_add(&name);
                    cx.notify();
                })),
            );
        }
        c::column()
            .p_1()
            .border_b_1()
            .border_color(t::panel_outline())
            .child(
                c::row()
                    .child(div().flex_none().px_1().text_color(t::text()).child(label))
                    .child(
                        c::button("nodes-save", "Save")
                            .flex_none()
                            .w(px(72.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save_nodes_file();
                                cx.notify();
                            })),
                    )
                    .child(
                        c::toggle(
                            "nodes-run-canvas",
                            if running { "Running…" } else { "Run" },
                            !running,
                        )
                        .flex_none()
                        .w(px(96.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_nodes(cx);
                            cx.notify();
                        })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .text_color(t::text_muted())
                            .child("Drag to pan · wheel to zoom · drag a port to wire"),
                    ),
            )
            .child(adds)
    }

    /// The canvas itself.
    pub(crate) fn nodes_view(&self, cx: &mut Context<'_, Self>) -> impl IntoElement + use<> {
        let scene = self.nodes_scene();
        div()
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .bg(t::window_bg())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.nodes_mouse_down(event.position, event.click_count);
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    if event.pressed_button == Some(MouseButton::Left)
                        && this.nodes_mouse_drag(event.position)
                    {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _, cx| {
                    this.nodes_mouse_up(event.position);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    this.nodes_scroll(event);
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    move |bounds, _, _| bounds,
                    move |_, bounds: Bounds<gpui::Pixels>, window, cx| {
                        *scene.bounds_slot.lock().unwrap_or_else(|e| e.into_inner()) = bounds;
                        window.with_content_mask(
                            Some(gpui::ContentMask { bounds }),
                            move |window| {
                                paint_nodes(&scene, bounds, window, cx);
                            },
                        );
                    },
                )
                .size_full(),
            )
    }

    /// Gathers the boxes, wires and the drag for one paint.
    fn nodes_scene(&self) -> NodesScene {
        let view: &NodesView = &self.models.graph_view;
        let Some(state) = self.models.graph.as_ref() else {
            return NodesScene {
                boxes: Vec::new(),
                wires: Vec::new(),
                pending: None,
                selected: None,
                rows: std::collections::BTreeMap::default(),
                viewport: view.viewport.clone(),
                problems: Vec::new(),
                bounds_slot: view.bounds.clone(),
            };
        };
        let boxes: Vec<NodeBox> = state
            .graph
            .nodes
            .iter()
            .map(|n| crate::edit::nodes::node_box(state, n))
            .collect();
        let index_of = |id: u32| boxes.iter().position(|b| b.id == id);
        let mut wires = Vec::new();
        for l in &state.graph.links {
            let (Some(a), Some(b)) = (index_of(l.from()), index_of(l.to())) else {
                continue;
            };
            let (Some(o), Some(i)) = (
                boxes[a].outputs.iter().position(|p| p.name == l.output()),
                boxes[b].inputs.iter().position(|p| p.name == l.input()),
            ) else {
                continue;
            };
            wires.push((a, o, b, i));
        }
        let origin = view.bounds.lock().unwrap_or_else(|e| e.into_inner()).origin;
        let pending = match &view.drag {
            Some(crate::edit::nodes::NodeDrag::Wire {
                from, output, to, ..
            }) => {
                let start = index_of(*from)
                    .and_then(|a| boxes[a].outputs.iter().find(|p| p.name == *output))
                    .map(|p| to_screen(&view.viewport, origin, p.at));
                start.map(|s| (s, gpui::point(px(px32(to.x)), px(px32(to.y)))))
            }
            _ => None,
        };
        NodesScene {
            boxes,
            wires,
            pending,
            selected: view.selected,
            rows: state.rows.clone(),
            viewport: view.viewport.clone(),
            problems: state
                .problems
                .iter()
                .map(|p| p.to_string().into())
                .collect(),
            bounds_slot: view.bounds.clone(),
        }
    }
}

/// A cubic between two ports with horizontal tangents: the shape a
/// node editor reader expects.
fn wire(
    a: Point<gpui::Pixels>,
    b: Point<gpui::Pixels>,
    width: f32,
) -> Option<gpui::Path<gpui::Pixels>> {
    let (ax, ay) = (f32::from(a.x), f32::from(a.y));
    let (bx, by) = (f32::from(b.x), f32::from(b.y));
    let dx = ((bx - ax).abs() * 0.5).clamp(24.0, 120.0);
    let mut pb = PathBuilder::stroke(px(width));
    pb.move_to(a);
    pb.cubic_bezier_to(
        b,
        gpui::point(px(ax + dx), px(ay)),
        gpui::point(px(bx - dx), px(by)),
    );
    pb.build().ok()
}

/// A rectangle path, for a fill or a stroke.
fn rect_path(
    origin: Point<gpui::Pixels>,
    w: f32,
    h: f32,
    builder: PathBuilder,
) -> Option<gpui::Path<gpui::Pixels>> {
    let (x, y) = (f32::from(origin.x), f32::from(origin.y));
    let mut pb = builder;
    pb.move_to(gpui::point(px(x), px(y)));
    pb.line_to(gpui::point(px(x + w), px(y)));
    pb.line_to(gpui::point(px(x + w), px(y + h)));
    pb.line_to(gpui::point(px(x), px(y + h)));
    pb.close();
    pb.build().ok()
}

/// A circle path from a polygon, enough sides that it reads round at
/// port size.
fn circle_path(
    center: Point<gpui::Pixels>,
    r: f32,
    builder: PathBuilder,
) -> Option<gpui::Path<gpui::Pixels>> {
    let (cx, cy) = (f32::from(center.x), f32::from(center.y));
    let mut pb = builder;
    let n = 20;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let p = gpui::point(px(cx + r * a.cos()), px(cy + r * a.sin()));
        if i == 0 {
            pb.move_to(p);
        } else {
            pb.line_to(p);
        }
    }
    pb.close();
    pb.build().ok()
}

/// Shortens text until it fits `max_w` pixels at `size`, with an
/// ellipsis when it had to.
fn clip_text(window: &mut Window, text: &str, size: f32, max_w: f32) -> String {
    let width = |window: &mut Window, s: &str| -> f32 {
        let shared: SharedString = s.to_string().into();
        let run = gpui::TextRun {
            len: shared.len(),
            font: window.text_style().font(),
            color: gpui::Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(
            window
                .text_system()
                .shape_line(shared, px(size), std::slice::from_ref(&run), None)
                .width,
        )
    };
    if width(window, text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let candidate: String = chars[..n].iter().collect::<String>() + "…";
        if width(window, &candidate) <= max_w {
            return candidate;
        }
    }
    "…".into()
}

/// Paints one line of text at `at`, top-left, in the interface font
/// at `size`.
fn paint_text(
    window: &mut Window,
    cx: &mut App,
    at: Point<gpui::Pixels>,
    text: &str,
    size: f32,
    color: gpui::Rgba,
) {
    if text.is_empty() {
        return;
    }
    let text: SharedString = text.to_string().into();
    let run = gpui::TextRun {
        len: text.len(),
        font: window.text_style().font(),
        color: color.into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text, px(size), std::slice::from_ref(&run), None);
    let _ = line.paint(
        at,
        px((size * 1.2).ceil()),
        gpui::TextAlign::Left,
        None,
        window,
        cx,
    );
}

/// Everything, bottom to top: wires, boxes, ports, text.
fn paint_nodes(
    scene: &NodesScene,
    bounds: Bounds<gpui::Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let origin = bounds.origin;
    let vp = &scene.viewport;
    let zoom = px32(vp.zoom);
    let sp = |p: kurbo::Point| to_screen(vp, origin, p);
    let stroke = f32::from(t::stroke()).max(1.0);
    let wire_w = (1.5 * zoom).max(1.0);

    // Wires first, under the boxes they join.
    for &(a, o, b, i) in &scene.wires {
        let from = sp(scene.boxes[a].outputs[o].at);
        let to = sp(scene.boxes[b].inputs[i].at);
        if let Some(p) = wire(from, to, wire_w) {
            window.paint_path(p, t::text_muted());
        }
    }
    if let Some((from, to)) = scene.pending
        && let Some(p) = wire(from, to, wire_w)
    {
        window.paint_path(p, t::text());
    }

    let text_px = (crate::workspace::UI_TEXT_PX * zoom).clamp(6.0, 40.0);
    let port_r = (px32(crate::edit::nodes::PORT_R) * zoom).max(2.0);
    for nb in &scene.boxes {
        let top_left = sp(nb.rect.origin());
        let w = px32(nb.rect.width()) * zoom;
        let h = px32(nb.rect.height()) * zoom;
        let header_h = px32(crate::edit::nodes::HEADER_H) * zoom;
        let selected = scene.selected == Some(nb.id);
        // The body, then the header band, then the keyline.
        if let Some(p) = rect_path(top_left, w, h, PathBuilder::fill()) {
            window.paint_path(p, t::field_bg());
        }
        if let Some(p) = rect_path(top_left, w, header_h, PathBuilder::fill()) {
            window.paint_path(
                p,
                if selected {
                    t::selected_bg()
                } else {
                    t::panel_bg()
                },
            );
        }
        if let Some(p) = rect_path(
            top_left,
            w,
            h,
            PathBuilder::stroke(px(if selected {
                f32::from(t::stroke_emphasis())
            } else {
                stroke
            })),
        ) {
            window.paint_path(
                p,
                if selected {
                    t::selected_bg()
                } else {
                    t::cell_border()
                },
            );
        }
        // Title left, status right, in the header.
        let pad = px32(crate::edit::nodes::PAD) * zoom;
        let title_ink = if selected {
            t::selected_ink()
        } else {
            t::text()
        };
        paint_text(
            window,
            cx,
            gpui::point(
                top_left.x + px(pad),
                top_left.y + px((header_h - text_px * 1.2) / 2.0),
            ),
            &nb.title,
            text_px,
            title_ink,
        );
        let mark = match scene.rows.get(&nb.id) {
            Some(RowState::Running(_)) => "…",
            Some(RowState::Done(Status::Ran, _)) => "✓",
            Some(RowState::Done(Status::Skipped, _)) => "=",
            Some(RowState::Done(Status::Failed, _)) => "✗",
            Some(RowState::Done(Status::Blocked, _)) => "–",
            _ => "",
        };
        paint_text(
            window,
            cx,
            gpui::point(
                top_left.x + px(w - pad - text_px),
                top_left.y + px((header_h - text_px * 1.2) / 2.0),
            ),
            mark,
            text_px,
            title_ink,
        );
        // Rows: an input's name at the left, an output's at the right.
        let row_h = px32(crate::edit::nodes::ROW_H) * zoom;
        let inner_w = w - 2.0 * pad - 2.0 * port_r;
        for port in &nb.inputs {
            let y = top_left.y
                + px(header_h
                    + pad / 2.0
                    + row_h * px32(port.row as f64)
                    + (row_h - text_px * 1.2) / 2.0);
            let label = match &port.value {
                Some(v) => format!("{} {v}", port.name),
                None => port.name.clone(),
            };
            let label = clip_text(window, &label, text_px, inner_w);
            paint_text(
                window,
                cx,
                gpui::point(top_left.x + px(pad + port_r), y),
                &label,
                text_px,
                if port.linked || port.value.is_some() {
                    t::text()
                } else {
                    t::text_muted()
                },
            );
        }
        for port in &nb.outputs {
            let y = top_left.y
                + px(header_h
                    + pad / 2.0
                    + row_h * px32(port.row as f64)
                    + (row_h - text_px * 1.2) / 2.0);
            // Right-aligned by measuring: the shaper knows the width.
            let text: SharedString = port.name.clone().into();
            let run = gpui::TextRun {
                len: text.len(),
                font: window.text_style().font(),
                color: t::text_muted().into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                text,
                px(text_px),
                std::slice::from_ref(&run),
                None,
            );
            let tw = f32::from(line.width);
            let _ = line.paint(
                gpui::point(top_left.x + px(w - pad - port_r - tw), y),
                px((text_px * 1.2).ceil()),
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        // Ports: a filled dot when wired, a ring when not.
        for port in nb.inputs.iter().chain(&nb.outputs) {
            let at = sp(port.at);
            if let Some(p) = circle_path(at, port_r, PathBuilder::fill()) {
                window.paint_path(
                    p,
                    if port.linked {
                        t::text()
                    } else {
                        t::field_bg()
                    },
                );
            }
            if let Some(p) = circle_path(at, port_r, PathBuilder::stroke(px(stroke))) {
                window.paint_path(p, t::text());
            }
        }
        // A result line under the box, when the node has one.
        if let Some(RowState::Done(_, Some(note))) = scene.rows.get(&nb.id) {
            paint_text(
                window,
                cx,
                gpui::point(top_left.x, top_left.y + px(h + pad / 2.0)),
                note,
                text_px * 0.9,
                t::text_muted(),
            );
        }
    }
    // Problems in the corner, so a file that will not run says why.
    let mut y = f32::from(origin.y) + 8.0;
    for p in &scene.problems {
        paint_text(
            window,
            cx,
            gpui::point(origin.x + px(8.0), px(y)),
            p,
            crate::workspace::UI_TEXT_PX,
            t::text(),
        );
        y += crate::workspace::UI_TEXT_PX * 1.4;
    }
}
