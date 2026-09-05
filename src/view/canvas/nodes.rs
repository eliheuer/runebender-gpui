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
use crate::view::paint::build_path;
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
use kurbo::BezPath;
use runebender_core::document::nodes_run::Status;
use runebender_core::ui::editing::viewport::ViewPort;
use runebender_core::ui::nodes as nl;

/// What one paint of the canvas needs, gathered while `self` is
/// borrowed and handed to the paint closure.
struct NodesScene {
    /// Every node, laid out.
    boxes: Vec<NodeBox>,
    /// The mark colour of each box's header, by box index.
    marks: Vec<Option<&'static str>>,
    /// `(from box index, output index, to box index, input index)`.
    wires: Vec<(usize, usize, usize, usize)>,
    /// A wire being dragged: from a port, to a window point.
    pending: Option<(Point<gpui::Pixels>, Point<gpui::Pixels>)>,
    /// What the dragged wire carries, so the inputs that take it light
    /// up.
    pending_kind: Option<runebender_core::document::nodes::Kind>,
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
    /// The strip above the canvas: one row. The files beside the font,
    /// New, Open…, Save, Run. Node types come from the right-click
    /// menu, and the same commands sit in the Nodes menu.
    pub(crate) fn nodes_strip(&self, cx: &mut Context<'_, Self>) -> gpui::Div {
        let Some(state) = self.models.graph.as_ref() else {
            return c::row();
        };
        let running = state.running;
        let open_path = state.path.clone();
        let mut row = c::row().p_1().border_b_1().border_color(t::panel_outline());
        for file in self.models.graph_files.clone() {
            let current = file == open_path;
            row = row.child(
                c::toggle(
                    SharedString::from(format!("nodes-tab-{}", file.display())),
                    crate::edit::nodes::file_label(&file),
                    current,
                )
                .flex_none()
                .px_2()
                .w_auto()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.models.graph.as_ref().is_none_or(|g| g.path != file) {
                        this.open_nodes_file(&file);
                        this.models.graph_view.selected = None;
                    }
                    cx.notify();
                })),
            );
        }
        if !self.models.graph_files.contains(&open_path) {
            row = row.child(
                c::toggle(
                    "nodes-tab-open",
                    crate::edit::nodes::file_label(&open_path),
                    true,
                )
                .flex_none()
                .px_2()
                .w_auto(),
            );
        }
        let button = |id: &'static str, label: &'static str| {
            c::button(id, label).flex_none().px_2().w_auto()
        };
        row.child(div().flex_1())
            .child(
                button("nodes-new", "New").on_click(cx.listener(|this, _, _, cx| {
                    this.new_nodes_file();
                    cx.notify();
                })),
            )
            .child(
                button("nodes-open-strip", "Open…").on_click(cx.listener(|this, _, _, cx| {
                    this.command_open_nodes_file(cx);
                })),
            )
            .child(
                button("nodes-save", "Save").on_click(cx.listener(|this, _, _, cx| {
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
                .px_2()
                .w_auto()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.run_nodes(cx);
                    cx.notify();
                })),
            )
            .children(self.nodes_choices(cx))
    }

    /// One toggle per choice for the selected Master, Model or Adapter
    /// node; None for any other node.
    fn nodes_choices(&self, cx: &mut Context<'_, Self>) -> Option<gpui::Div> {
        let state = self.models.graph.as_ref()?;
        let id = self.models.graph_view.selected?;
        let node = state.graph.node(id)?;
        let current = node
            .values
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from);
        let options: Vec<String> = match node.type_name.as_str() {
            "core.master" => self
                .project
                .as_ref()
                .map(|p| p.master_names.iter().map(|m| m.to_string()).collect())
                .unwrap_or_default(),
            "core.model" => Self::installed_models()
                .into_iter()
                .map(|(n, _)| n)
                .collect(),
            "core.adapter" => Self::installed_adapters()
                .into_iter()
                .map(|(n, _)| n)
                .collect(),
            _ => return None,
        };
        if options.is_empty() {
            return Some(
                c::row().child(
                    div()
                        .text_color(t::text_muted())
                        .child("Nothing installed to choose from"),
                ),
            );
        }
        let mut row = c::row().flex_wrap();
        for option in options {
            let on = current.as_deref() == Some(option.as_str());
            let value = option.clone();
            row = row.child(
                c::toggle(
                    SharedString::from(format!("nodes-choice-{option}")),
                    option.clone(),
                    on,
                )
                .flex_none()
                .px_2()
                .w_auto()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.nodes_set_value(id, "name", serde_json::Value::String(value.clone()));
                    cx.notify();
                })),
            );
        }
        Some(row)
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.nodes_context_menu(event.position);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    this.nodes_scroll(event);
                    cx.notify();
                }),
            )
            .children(self.nodes_menu_overlay(cx))
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

    /// The right-click menu: one row per node type that runs.
    fn nodes_menu_overlay(&self, cx: &mut Context<'_, Self>) -> Option<gpui::Stateful<gpui::Div>> {
        let (at, _) = self.models.graph_view.menu?;
        let state = self.models.graph.as_ref()?;
        let mut list = div()
            .id("nodes-menu")
            .absolute()
            .left(at.x)
            .top(at.y)
            .flex()
            .flex_col()
            .py_1()
            .bg(t::panel_bg())
            .border(t::stroke())
            .border_color(t::cell_border())
            .rounded(t::radius());
        for (i, ty) in state
            .registry
            .types
            .iter()
            .filter(|t| t.implemented)
            .enumerate()
        {
            let name = ty.name.clone();
            list = list.child(
                div()
                    .id(("nodes-menu-item", i))
                    .px_3()
                    .py_0p5()
                    .text_color(t::text())
                    .cursor_pointer()
                    .hover(|el| el.bg(t::cell_selected_bg()))
                    .child(ty.title.clone())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.nodes_add_from_menu(&name);
                        cx.notify();
                    })),
            );
        }
        Some(list)
    }

    /// Gathers the boxes, wires and the drag for one paint.
    fn nodes_scene(&self) -> NodesScene {
        let view: &NodesView = &self.models.graph_view;
        let Some(state) = self.models.graph.as_ref() else {
            return NodesScene {
                boxes: Vec::new(),
                marks: Vec::new(),
                wires: Vec::new(),
                pending: None,
                pending_kind: None,
                selected: None,
                rows: std::collections::BTreeMap::default(),
                viewport: view.viewport.clone(),
                problems: Vec::new(),
                bounds_slot: view.bounds.clone(),
            };
        };
        let boxes: Vec<NodeBox> = nl::layout(&state.graph, &state.registry);
        let marks: Vec<Option<&'static str>> = boxes.iter().map(NodeBox::mark).collect();
        let index_of = |id: u32| boxes.iter().position(|b| b.id == id);
        let wires = nl::wires(&state.graph, &boxes);
        let origin = view.bounds.lock().unwrap_or_else(|e| e.into_inner()).origin;
        let pending_kind = match &view.drag {
            Some(crate::edit::nodes::NodeDrag::Wire { kind, .. }) => Some(*kind),
            _ => None,
        };
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
            marks,
            wires,
            pending,
            pending_kind,
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

use runebender_core::ui::nodes::{circle, wire_path};

/// A rectangle as a path, in canvas units.
fn rect(r: kurbo::Rect) -> BezPath {
    use kurbo::Shape as _;
    r.to_path(0.1)
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
///
/// Geometry is kurbo in canvas units and goes through the same
/// `build_path` and affine the editing view uses for an outline, so
/// a wire and a glyph contour rasterize the same way. Text is placed
/// in pixels through the same mapping.
fn paint_nodes(
    scene: &NodesScene,
    bounds: Bounds<gpui::Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let origin = bounds.origin;
    let vp = &scene.viewport;
    let zoom = px32(vp.zoom);
    // Canvas units (Y down) to canvas pixels: the viewport's affine,
    // which flips Y, after a flip that puts the file's Y-down space
    // into the viewport's Y-up one.
    let tf = nl::canvas_affine(vp);
    let sp = |p: kurbo::Point| to_screen(vp, origin, p);
    let stroke = f32::from(t::stroke()).max(1.0);
    let wire_w = (1.5 * zoom).max(1.0);
    let draw = |window: &mut Window, path: &BezPath, builder: PathBuilder, color: gpui::Rgba| {
        if let Some(p) = build_path(path, tf, origin, builder) {
            window.paint_path(p, color);
        }
    };

    // The dot grid: a small filled dot at every pitch, in canvas units
    // so it zooms with the boxes. Node edges run through the dot
    // centres, since the box sizes are multiples of the pitch and
    // positions snap to it. Skipped when the pitch is too small to
    // read.
    if px32(nl::GRID) * zoom >= 8.0 {
        let local = kurbo::Rect::new(
            0.0,
            0.0,
            f64::from(f32::from(bounds.size.width)),
            f64::from(f32::from(bounds.size.height)),
        );
        let rings = nl::grid_rings(nl::visible_canvas(vp, local));
        // Quiet: the keyline mixed most of the way into the ground,
        // so the grid reads and still sits behind the boxes. Mixed,
        // not alpha, because a path's alpha is not reliable here.
        let (bg, line) = (t::window_bg(), t::cell_border());
        let mix = |a: f32, b: f32| a + (b - a) * 0.4;
        let faint = gpui::Rgba {
            r: mix(bg.r, line.r),
            g: mix(bg.g, line.g),
            b: mix(bg.b, line.b),
            a: 1.0,
        };
        draw(window, &rings, PathBuilder::fill(), faint);
    }

    // A wire takes the mark colour of what it carries; a value with
    // no colour is drawn in ink. Each is drawn twice: a keyline the
    // colour of a cell's outline, one stroke wider on each side, then
    // the colour on top, so it reads on the grey the way a cell does.
    let wire_ink = |kind| {
        nl::kind_mark(kind)
            .and_then(|m| t::mark_paint(Some(m)))
            .map_or_else(t::text_muted, |p| p.bg.unwrap_or(p.border))
    };
    let draw_wire = |window: &mut Window, path: &BezPath, ink| {
        draw(
            window,
            path,
            PathBuilder::stroke(px(wire_w + 2.0 * stroke)),
            t::cell_border(),
        );
        draw(window, path, PathBuilder::stroke(px(wire_w)), ink);
    };
    for &(a, o, b, i) in &scene.wires {
        let port = &scene.boxes[a].outputs[o];
        let path = wire_path(port.at, scene.boxes[b].inputs[i].at);
        draw_wire(window, &path, wire_ink(port.kind));
    }
    if let Some((from, to)) = scene.pending {
        // The pending end is a window point; bring it into canvas
        // units so the wire is one path like the others.
        let inverse = tf.inverse();
        let to_canvas = |p: Point<gpui::Pixels>| {
            inverse
                * kurbo::Point::new(
                    f64::from(f32::from(p.x - origin.x)),
                    f64::from(f32::from(p.y - origin.y)),
                )
        };
        let path = wire_path(to_canvas(from), to_canvas(to));
        draw_wire(
            window,
            &path,
            scene.pending_kind.map_or_else(t::text, wire_ink),
        );
    }

    let text_px = (crate::workspace::UI_TEXT_PX * zoom).clamp(6.0, 40.0);
    let port_r = (px32(nl::PORT_R) * zoom).max(2.0);
    for (index, nb) in scene.boxes.iter().enumerate() {
        let mark = scene
            .marks
            .get(index)
            .copied()
            .flatten()
            .and_then(|m| t::mark_paint(Some(m)));
        let top_left = sp(nb.rect.origin());
        let w = px32(nb.rect.width()) * zoom;
        let h = px32(nb.rect.height()) * zoom;
        let header_h = px32(nl::HEADER_H) * zoom;
        let selected = scene.selected == Some(nb.id);
        let header_rect = nb.header();
        let outline = if selected {
            t::selected_bg()
        } else {
            mark.as_ref().map_or_else(t::cell_border, |m| m.border)
        };
        // The body, the header band in the node's mark colour (the way
        // a grid cell wears its glyph's, inverted when selected), the
        // rule between them, then the keyline.
        draw(window, &rect(nb.rect), PathBuilder::fill(), t::field_bg());
        let header_bg = if selected {
            t::selected_bg()
        } else {
            mark.as_ref().and_then(|m| m.bg).unwrap_or_else(t::panel_bg)
        };
        draw(window, &rect(header_rect), PathBuilder::fill(), header_bg);
        let mut rule = BezPath::new();
        rule.move_to(kurbo::Point::new(header_rect.x0, header_rect.y1));
        rule.line_to(kurbo::Point::new(header_rect.x1, header_rect.y1));
        draw(window, &rule, PathBuilder::stroke(px(stroke)), outline);
        draw(
            window,
            &rect(nb.rect),
            PathBuilder::stroke(px(if selected {
                f32::from(t::stroke_emphasis())
            } else {
                stroke
            })),
            outline,
        );
        // Title left, status right, in the header.
        let pad = px32(nl::PAD) * zoom;
        let title_ink = if selected {
            t::selected_ink()
        } else {
            mark.as_ref().map_or_else(t::text, |m| m.ink)
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
        let row_h = px32(nl::ROW_H) * zoom;
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
        // Ports: filled with the wire's colour when wired, the field
        // ground when not, keylined in ink either way. While a wire
        // is being dragged, the inputs that take it grow a second
        // ring and the rest fade, so the legal drops show.
        for (port, is_input) in nb
            .inputs
            .iter()
            .map(|p| (p, true))
            .chain(nb.outputs.iter().map(|p| (p, false)))
        {
            let (takes, fades) = match scene.pending_kind {
                Some(k) if is_input => (port.kind == k, port.kind != k),
                Some(_) => (false, true),
                None => (false, false),
            };
            let ink = if fades { t::text_muted() } else { t::text() };
            let dot = circle(port.at, nl::PORT_R);
            let fill = if port.linked {
                wire_ink(port.kind)
            } else {
                t::field_bg()
            };
            draw(window, &dot, PathBuilder::fill(), fill);
            draw(window, &dot, PathBuilder::stroke(px(stroke)), ink);
            if takes {
                let ring = circle(port.at, nl::PORT_R * 2.0);
                draw(window, &ring, PathBuilder::stroke(px(stroke)), t::text());
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
