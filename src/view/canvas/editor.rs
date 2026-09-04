// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The glyph editing canvas.
//!
//! `editor_view` gathers everything the canvas draws into an
//! `EditorScene`, then hands it to `paint_scene`, which paints one
//! layer after another. Each layer is one function in this file,
//! named for what it paints, called in draw order.

use crate::Arc;
use crate::Mutex;
use crate::Workspace;
use crate::view::paint::build_fill_path;
use crate::view::paint::build_path;
use crate::view::paint::paint_batched;
use crate::view::render::px32;
use crate::view::render::{to_byte, to_index};
use crate::view::theme as t;
use crate::workspace::Drag;
use crate::workspace::HIT_RADIUS_PX;
use crate::workspace::MeasureOpts;
use crate::workspace::Tool;
use gpui::App;
use gpui::Bounds;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::MouseButton;
use gpui::ParentElement;
use gpui::PathBuilder;
use gpui::Point;
use gpui::Styled;
use gpui::Window;
use gpui::canvas;
use gpui::div;
use gpui::px;
use kurbo::Affine;
use kurbo::BezPath;
use runebender_core::document::project::GlyphPoint;
use runebender_core::formats::color_font::read_color_mapping;
use runebender_core::formats::color_font::read_color_palette;
use runebender_core::formats::lib_keys::Annotation;
use runebender_core::formats::lib_keys::hoi_quad_at;
use runebender_core::formats::lib_keys::read_annotations;
use runebender_core::formats::lib_keys::read_hoi_intermediates;
use runebender_core::formats::lib_keys::read_masks;
use runebender_core::ui::editing::ViewPort;
use std::collections::HashSet;

/// A contour start marker: its point, its direction, and whether the
/// contour closes.
type StartMarker = ((f64, f64), (f64, f64), bool);
/// The pen tool's preview: last on-curve point, pointer, and the ring
/// on the start point when closing would land.
type PenPreview = ((f64, f64), (f64, f64), Option<(f64, f64)>);
/// The shapes-tool drag: its corners and whether it is an ellipse.
type ShapePreview = ((f64, f64), (f64, f64), bool);
/// The knife drag: its two ends and the contour intersections.
type KnifeLine = ((f64, f64), (f64, f64), Vec<kurbo::Point>);
/// A HOI knob: the node it belongs to and its intermediate point.
type HoiKnob = ((usize, usize), (f64, f64));
/// One curvature comb strip, sampled along a segment.
type CombStrip = Vec<runebender_core::analysis::curve::CombSample>;
/// The measure HUD in design space: colorized strokes, measurements,
/// and side bearings.
type MeasureHud = (
    Vec<runebender_core::analysis::measure::ColoredStroke>,
    Vec<runebender_core::analysis::measure::Measurement>,
    Option<runebender_core::analysis::measure::SideBearings>,
);
/// A path batch keyed by colour: the colour and the paths that share it.
type ColorBatch = std::collections::BTreeMap<u32, (gpui::Rgba, Vec<BezPath>)>;

/// One sort of the text buffer: its fill, its quiet metric box, and
/// its corner marks.
///
/// The active sort's fill paints too while the text tool is up. The
/// corner marks turn kern-colored during a kern drag. Coordinates are
/// relative to the active sort. The web editor draws sorts the same
/// way.
struct SortPaint {
    /// The sort's outline, if it names a glyph.
    path: Option<Arc<BezPath>>,
    /// Left edge, relative to the active sort.
    x: f64,
    /// Baseline, relative to the active sort.
    y: f64,
    /// Advance width in design units.
    advance: f64,
    /// True for the sort being edited.
    active: bool,
    /// 0 = normal, 1 = kern-active, 2 = kern-previous.
    kern: u8,
}

/// Everything the editor canvas draws for one glyph, gathered from
/// the workspace before painting so the paint closure owns plain
/// data and no borrow of `self`.
struct EditorScene {
    /// The tracing template and its placement rect in design space.
    glyph_image: Option<(Arc<gpui::RenderImage>, kurbo::Rect)>,
    /// The glyph's own contours, or the interpolated instance.
    outline: Arc<BezPath>,
    /// The resolved components.
    component_path: Arc<BezPath>,
    /// True while the text tool is up.
    text_mode: bool,
    /// The text buffer's sorts, ready to paint.
    sort_paints: Vec<SortPaint>,
    /// The text caret, when the text tool is up.
    text_caret: Option<(f64, f64)>,
    /// Top of a sort's metric box.
    sort_top: f64,
    /// Bottom of a sort's metric box.
    sort_bottom: f64,
    /// Other masters toggled visible in the Layers section.
    reference_paths: Vec<Arc<BezPath>>,
    /// An interpolated ghost outline. Not produced today.
    ghost: Option<Arc<BezPath>>,
    /// Every control point of the glyph's own contours.
    points: Arc<Vec<GlyphPoint>>,
    /// Where each closed contour starts and which way it runs.
    start_markers: Vec<StartMarker>,
    /// Anchors as `(name, x, y)`.
    anchors: Arc<Vec<(Arc<str>, f64, f64)>>,
    /// Indices of the selected anchors.
    selected_anchors: Vec<usize>,
    /// Advance width.
    advance: f64,
    /// Ascender.
    ascender: f64,
    /// Descender.
    descender: f64,
    /// Units per em.
    upm: f64,
    /// X-height, when the font has one.
    x_height: Option<f64>,
    /// Cap height, when the font has one.
    cap_height: Option<f64>,
    /// Alignment zones as `(lo, hi)` pairs.
    zones: Vec<(f64, f64)>,
    /// Node trajectories across the axis, one per node.
    trajectories: Option<Vec<Vec<kurbo::Point>>>,
    /// Marks placed on this glyph's anchors.
    mark_cloud: Vec<Arc<BezPath>>,
    /// Mask contours.
    mask_paths: Vec<Arc<BezPath>>,
    /// Working marks pinned to design-space points.
    annotations: Vec<Annotation>,
    /// HOI knobs, one per node.
    hoi_knobs: Vec<HoiKnob>,
    /// The HOI knob being dragged and its live point.
    hoi_live: Option<((usize, usize), (f64, f64))>,
    /// The ends of the HOI curve being dragged.
    hoi_drag_ends: Option<((f64, f64), (f64, f64))>,
    /// The hot guide: hovered or mid-drag, as `(local, index)`.
    guide_hot: Option<(bool, usize)>,
    /// Every guide, global then local, as `(local, line)`.
    guides: Vec<(bool, norad::Line)>,
    /// Top of the metric box.
    box_top: f64,
    /// Bottom of the metric box.
    box_bottom: f64,
    /// The design-to-canvas transform from the editor state.
    transform: Affine,
    /// The zoom from the editor state.
    zoom: f64,
    /// The selected points as `(contour, index)`.
    selected_points: HashSet<(usize, usize)>,
    /// The locked points as `(contour, index)`.
    locked_points: HashSet<(usize, usize)>,
    /// The marquee drag's corners.
    marquee: Option<((f64, f64), (f64, f64))>,
    /// The free-transform box around a multi-point selection.
    transform_box: Option<kurbo::Rect>,
    /// The shapes-tool drag: corners and whether it is an ellipse.
    shape_preview: Option<ShapePreview>,
    /// The measure-tool drag line.
    measure_line: Option<((f64, f64), (f64, f64))>,
    /// Curvature comb strips.
    comb_strips: Vec<CombStrip>,
    /// The largest curvature in the comb, for the gradient.
    comb_maxk: f64,
    /// Continuity rings: the node and its colour.
    continuity_rings: Vec<(kurbo::Point, gpui::Rgba)>,
    /// The measure panel's switches.
    measure_opts: MeasureOpts,
    /// Every segment's own bounding box, for the size labels.
    segment_boxes: Vec<kurbo::Rect>,
    /// The measure HUD, when any of its switches is on.
    measure_hud: Option<MeasureHud>,
    /// The background layer's outline.
    background_path: Option<Arc<BezPath>>,
    /// Stacked color layers, bottom first, with their palette colour.
    color_preview: Vec<(Arc<BezPath>, gpui::Rgba)>,
    /// Visible per-glyph layers.
    glyph_layer_paths: Vec<Arc<BezPath>>,
    /// The reference glyph.
    reference_path: Option<Arc<BezPath>>,
    /// The alt-hovered segment.
    hover_seg: Option<kurbo::PathSeg>,
    /// The sidebearing edge under the pointer, `true` for the right.
    sidebearing_hover: Option<bool>,
    /// True when a component is selected.
    component_selected: bool,
    /// The pen rubber band.
    pen_preview: Option<PenPreview>,
    /// The knife drag.
    knife_line: Option<KnifeLine>,
    /// True when the glyph draws filled with no editable chrome.
    preview_mode: bool,
    /// The design grid as lines rather than dots.
    grid_lines: bool,
    /// Where the paint closure records the canvas bounds.
    bounds_slot: Arc<Mutex<Bounds<gpui::Pixels>>>,
    /// True on the first paint after opening, when the glyph is fit.
    needs_fit: bool,
}

/// The design-to-screen mapping for one paint: the transform after
/// any first-paint fit, the zoom that goes with it, and the canvas.
struct Screen {
    /// Design space to canvas pixels.
    transform: Affine,
    /// The zoom that goes with `transform`.
    zoom: f64,
    /// The canvas origin in window pixels.
    origin: Point<gpui::Pixels>,
    /// The canvas bounds in window pixels.
    bounds: Bounds<gpui::Pixels>,
}

impl Screen {
    /// Builds the mapping for one paint. On the first paint after
    /// opening the glyph is fit locally; the entity state is fitted
    /// on the next mouse interaction via the same bounds slot.
    fn new(scene: &EditorScene, bounds: Bounds<gpui::Pixels>) -> Self {
        let mut transform = scene.transform;
        let mut zoom = scene.zoom;
        if scene.needs_fit {
            let h: f32 = bounds.size.height.into();
            let w: f32 = bounds.size.width.into();
            let mut vp = ViewPort::new();
            vp.fit_to_canvas(
                w as f64,
                h as f64,
                scene.advance,
                scene.ascender,
                scene.descender,
                0.62,
            );
            transform = vp.affine();
            zoom = vp.zoom;
        }
        Self {
            transform,
            zoom,
            origin: bounds.origin,
            bounds,
        }
    }

    /// Maps a design-space point to window pixels.
    fn to_screen(&self, x: f64, y: f64) -> Point<gpui::Pixels> {
        let p = self.transform * kurbo::Point::new(x, y);
        gpui::point(self.origin.x + px(px32(p.x)), self.origin.y + px(px32(p.y)))
    }
}

impl Workspace {
    /// Gathers everything the editor canvas draws for glyph `index`.
    fn editor_scene(&self, index: usize) -> EditorScene {
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
        let font = self
            .font()
            .expect("the editor is only built while a font is open");
        let entry = &font.glyphs[index];
        let outline = entry.contour_path.clone();
        let component_path = entry.component_path.clone();
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
            let caret = text_mode.then_some((layout.cursor_x - off.0, layout.cursor_y - off.1));
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
        let start_markers: Vec<StartMarker> = font
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
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[a, b]| (a.min(*b), a.max(*b)))
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
        // this glyph's anchors, ghosted in place, the crowding check
        // while positioning anchors.
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
                        g.contours.get(ci).map(|c| {
                            Arc::new(runebender_core::outline::glyph_paths::contour_to_bezpath(c))
                        })
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
        let hoi_knobs: Vec<HoiKnob> = (self.show_trajectories)
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
        let comb_strips: Vec<CombStrip> = if self.curve_comb && self.editor.tool != Tool::Preview {
            font.font
                .get_glyph(entry.name.as_ref())
                .map(|g| {
                    let cubics = runebender_core::analysis::curve::cubics_from_norad(g);
                    let maxk = runebender_core::analysis::curve::max_curvature(&cubics);
                    if maxk <= 1e-12 {
                        (Vec::new(), 0.0)
                    } else {
                        (
                            runebender_core::analysis::curve::curvature_comb(
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
                        let cubics = runebender_core::analysis::curve::cubics_from_norad(g);
                        runebender_core::analysis::curve::node_continuity(&cubics)
                            .into_iter()
                            .filter_map(|nc| {
                                use runebender_core::analysis::curve::GLevel;
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
        // paint layer maps them to the screen and draws dimension
        // lines + labels.
        let measure_opts = self.measure_opts;
        // Every segment's own bounding box, for the size labels.
        let segment_boxes: Vec<kurbo::Rect> = if self.measure_opts.sizes {
            use kurbo::Shape as _;
            font.font
                .get_glyph(entry.name.as_ref())
                .map(|g| {
                    runebender_core::outline::segment_ops::segments(g)
                        .into_iter()
                        .map(|hit| hit.seg.bounding_box())
                        .filter(|b| b.width() >= 1.0 || b.height() >= 1.0)
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let measure_hud: Option<MeasureHud> = if measure_opts.any()
            && self.editor.tool != Tool::Preview
        {
            font.font.get_glyph(entry.name.as_ref()).map(|g| {
                use runebender_core::analysis::measure;
                use runebender_core::outline::path::hyper_model::Contour as WContour;
                let paths: Vec<runebender_core::outline::path::Path> = g
                    .contours
                    .iter()
                    .map(|c| {
                        runebender_core::outline::path::Path::from_contour(&WContour::from_norad(c))
                    })
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
                        .map(|g| {
                            Arc::new(runebender_core::outline::glyph_paths::contours_to_bezpath(
                                g,
                            ))
                        })
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
                        Arc::new(runebender_core::outline::glyph_paths::contours_to_bezpath(
                            glyph,
                        )),
                        gpui::Rgba {
                            r: px32(c[0]),
                            g: px32(c[1]),
                            b: px32(c[2]),
                            a: px32(c[3]),
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
            .map(|g| {
                Arc::new(runebender_core::outline::glyph_paths::contours_to_bezpath(
                    g,
                ))
            })
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
        let pen_preview: Option<PenPreview> = (|| {
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
        let knife_line: Option<KnifeLine> = match &self.editor.drag {
            Some(Drag::Knife { start, current }) => {
                let hits = font
                    .font
                    .get_glyph(entry.name.as_ref())
                    .map(|g| {
                        runebender_core::outline::knife::knife_hit_points(
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

        EditorScene {
            glyph_image,
            outline,
            component_path,
            text_mode,
            sort_paints,
            text_caret,
            sort_top,
            sort_bottom,
            reference_paths,
            ghost,
            points,
            start_markers,
            anchors,
            selected_anchors,
            advance,
            ascender,
            descender,
            upm,
            x_height,
            cap_height,
            zones,
            trajectories,
            mark_cloud,
            mask_paths,
            annotations,
            hoi_knobs,
            hoi_live,
            hoi_drag_ends,
            guide_hot,
            guides,
            box_top,
            box_bottom,
            transform,
            zoom,
            selected_points,
            locked_points,
            marquee,
            transform_box,
            shape_preview,
            measure_line,
            comb_strips,
            comb_maxk,
            continuity_rings,
            measure_opts,
            segment_boxes,
            measure_hud,
            background_path,
            color_preview,
            glyph_layer_paths,
            reference_path,
            hover_seg,
            sidebearing_hover,
            component_selected,
            pen_preview,
            knife_line,
            preview_mode,
            grid_lines: self.grid_lines,
            bounds_slot,
            needs_fit,
        }
    }

    /// The glyph editing canvas: the scene for glyph `index`, painted
    /// through one canvas element, with the mouse handlers and the
    /// info panel around it.
    pub(crate) fn editor_view(
        &self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let scene = self.editor_scene(index);

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
                        *scene.bounds_slot.lock().expect("the canvas bounds lock") = bounds;
                        // Everything the editor draws is clipped to
                        // the canvas: without a mask the outline and
                        // the neighbouring sorts paint straight over
                        // the header and the panels beside it.
                        window.with_content_mask(
                            Some(gpui::ContentMask { bounds }),
                            move |window| {
                                let screen = Screen::new(&scene, bounds);
                                paint_scene(&scene, &screen, window, cx);
                            },
                        );
                    },
                )
                .size_full(),
            )
            .child(self.editor_info_panel(index, cx))
    }
}

/// Paints every layer of the scene, bottom to top.
fn paint_scene(scene: &EditorScene, s: &Screen, window: &mut Window, cx: &mut App) {
    paint_design_grid(scene, s, window);
    if !scene.text_mode {
        paint_background_image(scene, s, window);
        paint_zones(scene, s, window);
        paint_color_preview(scene, s, window);
        paint_metrics(scene, s, window);
        paint_guides(scene, s, window);
        paint_hoi_knobs(scene, s, window);
        paint_trajectories(scene, s, window);
        paint_sidebearings(scene, s, window);
    }
    paint_preview_fill(scene, s, window);
    paint_sort_boxes(scene, s, window);
    paint_sort_fills(scene, s, window);
    paint_text_caret(scene, s, window);
    paint_reference_layers(scene, s, window);
    paint_components(scene, s, window);
    paint_background_layers(scene, s, window);
    paint_mark_cloud(scene, s, window);
    paint_masks(scene, s, window);
    paint_curvature_comb(scene, s, window);
    paint_outline(scene, s, window);
    paint_handles(scene, s, window);
    paint_points(scene, s, window);
    paint_start_markers(scene, s, window);
    paint_anchors(scene, s, window);
    paint_tool_preview(scene, s, window);
    paint_measure_hud(scene, s, window, cx);
    paint_continuity_rings(scene, s, window);
    paint_annotations(scene, s, window, cx);
    paint_transform_box(scene, s, window);
    paint_marquee(scene, s, window);
}

/// Hermite ease from 0 to 1.
fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// The design grid's two alphas at this zoom: the 8-unit lattice
/// fades in past 0.8x, the 2-unit fine grid past 8x.
fn grid_alphas(zoom: f64) -> (f64, f64) {
    let mid = smoothstep(((zoom - 0.8) / 0.8).clamp(0.0, 1.0));
    let close = smoothstep(((zoom - 8.0) / 8.0).clamp(0.0, 1.0));
    (mid, close)
}

/// The point scale at this zoom.
///
/// Simplified from the web editor's `point_scale` curve. Device
/// scale is 1 here.
fn point_scale(zoom: f64) -> f64 {
    if zoom <= 0.8 {
        0.72 + (1.0 - 0.72) * smoothstep((zoom / 0.8).clamp(0.0, 1.0))
    } else if zoom <= 8.0 {
        1.0 + 0.6 * smoothstep(((zoom - 0.8) / 7.2).clamp(0.0, 1.0))
    } else {
        1.6 + 0.8 * smoothstep(((zoom - 8.0) / 20.0).clamp(0.0, 1.0))
    }
}

/// Point chrome widths at this zoom: the point scale, the ring width,
/// and the halo width.
fn point_widths(zoom: f64) -> (f32, f32, f32) {
    let ps = px32(point_scale(zoom));
    let ring_w = (1.5 * ps).max(1.0);
    let halo_w = ring_w + 2.0;
    (ps, ring_w, halo_w)
}

/// The origin for paths already in window pixels.
fn zero() -> Point<gpui::Pixels> {
    gpui::point(px(0.0), px(0.0))
}

/// The batch key for a colour. An `Rgba` is not hashable, so its
/// bytes stand in.
fn color_key(c: gpui::Rgba) -> u32 {
    u32::from_be_bytes([
        to_byte(c.r * 255.0),
        to_byte(c.g * 255.0),
        to_byte(c.b * 255.0),
        to_byte(c.a * 255.0),
    ])
}

/// Fills a circle at a window-pixel centre.
fn paint_circle(window: &mut Window, center: Point<gpui::Pixels>, r: f32, color: gpui::Rgba) {
    use kurbo::Shape;
    let cx_: f32 = center.x.into();
    let cy_: f32 = center.y.into();
    let shape = kurbo::Circle::new((cx_ as f64, cy_ as f64), r as f64).to_path(0.25);
    if let Some(p) = build_fill_path(&shape, Affine::IDENTITY, zero()) {
        window.paint_path(p, color);
    }
}

/// A sort's metric box height on screen, at least one pixel.
fn sort_height_px(scene: &EditorScene, zoom: f64) -> f64 {
    ((scene.sort_top - scene.sort_bottom).max(1.0) * zoom).max(1.0)
}

/// Strokes one line between two window-pixel points.
fn paint_line(
    a: Point<gpui::Pixels>,
    b: Point<gpui::Pixels>,
    color: gpui::Rgba,
    window: &mut Window,
) {
    let mut pb = PathBuilder::stroke(px(1.0));
    pb.move_to(a);
    pb.line_to(b);
    if let Ok(p) = pb.build() {
        window.paint_path(p, color);
    }
}

/// The zoom-dependent design grid, behind everything: a dot at each
/// intersection, and the eye supplies the lines.
///
/// The 8-unit lattice fades in past 0.8x. Past 8x a 2-unit fine grid
/// joins underneath, so the 8s stay one grid at every zoom. Design
/// space here is sort-relative, so the grid is anchored at the active
/// sort's origin and the baseline lands on a row of dots. Lines were
/// the web editor's `draw_design_grid`; zoomed out they became a
/// mesh over the whole canvas, and dots carry the same information
/// without the weight.
fn paint_design_grid(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let (grid_mid_alpha, _) = grid_alphas(s.zoom);
    if !scene.preview_mode && grid_mid_alpha > 0.0 {
        let transform = s.transform;
        let bounds = s.bounds;
        let inv = transform.inverse();
        let bw: f32 = bounds.size.width.into();
        let bh: f32 = bounds.size.height.into();
        let c0 = inv * kurbo::Point::new(0.0, 0.0);
        let c1 = inv * kurbo::Point::new(bw as f64, bh as f64);
        let (min_x, max_x) = (c0.x.min(c1.x), c0.x.max(c1.x));
        let (min_y, max_y) = (c0.y.min(c1.y), c0.y.max(c1.y));
        // `skip_every` leaves out the intersections a coarser level
        // already drew. Dots are batched in runs: one path per few
        // thousand keeps under gpui's vertex limit per path.
        let level = |spacing: f64,
                     skip_every: u64,
                     size_px: f32,
                     color: gpui::Rgba,
                     window: &mut Window| {
            const RUN: usize = 2000;
            let mut pb = PathBuilder::fill();
            let mut count = 0;
            let flush = |pb: &mut PathBuilder, window: &mut Window| {
                let done = std::mem::replace(pb, PathBuilder::fill());
                if let Ok(p) = done.build() {
                    window.paint_path(p, color);
                }
            };
            let xs = to_index((min_x / spacing).floor())..=to_index((max_x / spacing).ceil());
            let ys = to_index((min_y / spacing).floor())..=to_index((max_y / spacing).ceil());
            for ix in xs {
                for iy in ys.clone() {
                    if skip_every > 0
                        && ix.unsigned_abs() % skip_every == 0
                        && iy.unsigned_abs() % skip_every == 0
                    {
                        continue;
                    }
                    let at = s.to_screen(ix as f64 * spacing, iy as f64 * spacing);
                    dot(&mut pb, at, size_px);
                    count += 1;
                    if count % RUN == 0 {
                        flush(&mut pb, window);
                    }
                }
            }
            if count % RUN != 0 {
                flush(&mut pb, window);
            }
        };
        // Lines, the web editor's grid, for those who want it: the
        // View > Grid menu.
        let lines = |spacing: f64,
                     skip_every: u64,
                     width_px: f32,
                     color: gpui::Rgba,
                     window: &mut Window| {
            let mut pb = PathBuilder::stroke(px(width_px));
            for ix in to_index((min_x / spacing).floor())..=to_index((max_x / spacing).ceil()) {
                if skip_every > 0 && ix.unsigned_abs() % skip_every == 0 {
                    continue;
                }
                let x = ix as f64 * spacing;
                pb.move_to(s.to_screen(x, min_y));
                pb.line_to(s.to_screen(x, max_y));
            }
            for iy in to_index((min_y / spacing).floor())..=to_index((max_y / spacing).ceil()) {
                if skip_every > 0 && iy.unsigned_abs() % skip_every == 0 {
                    continue;
                }
                let y = iy as f64 * spacing;
                pb.move_to(s.to_screen(min_x, y));
                pb.line_to(s.to_screen(max_x, y));
            }
            if let Ok(p) = pb.build() {
                window.paint_path(p, color);
            }
        };
        let coarse = t::design_grid_coarse(px32(grid_mid_alpha));
        let close_alpha = smoothstep(((s.zoom - 8.0) / 8.0).clamp(0.0, 1.0));
        let fine = t::design_grid_fine(px32(close_alpha));
        if scene.grid_lines {
            lines(8.0, 0, 1.0, coarse, window);
            if close_alpha > 0.0 {
                // The 2s only; every 4th line is an 8 the mid pass
                // already drew.
                lines(2.0, 4, 0.5, fine, window);
            }
        } else {
            level(8.0, 0, 1.5, coarse, window);
            if close_alpha > 0.0 {
                // The 2s only; every 4th intersection is an 8 the mid
                // pass already drew.
                level(2.0, 4, 1.0, fine, window);
            }
        }
    }
}

/// One grid dot: a square `size` pixels across, centred on `at`.
/// A square, not a circle, because a circle costs sixteen vertices
/// and a grid has tens of thousands of dots.
fn dot(pb: &mut PathBuilder, at: Point<gpui::Pixels>, size: f32) {
    let h = px(size / 2.0);
    pb.move_to(gpui::point(at.x - h, at.y - h));
    pb.line_to(gpui::point(at.x + h, at.y - h));
    pb.line_to(gpui::point(at.x + h, at.y + h));
    pb.line_to(gpui::point(at.x - h, at.y + h));
    pb.close();
}

/// The tracing template, under everything.
fn paint_background_image(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if let Some((image, rect)) = &scene.glyph_image {
        let a = s.to_screen(rect.x0, rect.y0);
        let b = s.to_screen(rect.x1, rect.y1);
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
}

/// Alignment zone bands.
fn paint_zones(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let bounds = s.bounds;
    for &(lo, hi) in &scene.zones {
        let a = s.to_screen(0.0, hi);
        let b = s.to_screen(0.0, lo);
        window.paint_quad(gpui::fill(
            Bounds::from_corners(
                gpui::point(bounds.origin.x, a.y),
                gpui::point(bounds.origin.x + bounds.size.width, b.y),
            ),
            t::zone_band(),
        ));
    }
}

/// The color stack, bottom first, so editing happens over the
/// composite.
fn paint_color_preview(scene: &EditorScene, s: &Screen, window: &mut Window) {
    for (path, color) in &scene.color_preview {
        if let Some(p) = build_fill_path(path, s.transform, s.origin) {
            window.paint_path(p, *color);
        }
    }
}

/// Every metric line the font defines.
///
/// The baseline always, then the box edges, the upm, ascender,
/// descender, x-height and cap-height, deduplicated. The web editor
/// draws the same set.
fn paint_metrics(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let hline = |y: f64, window: &mut Window| {
        let a = s.to_screen(0.0, y);
        let b = s.to_screen(scene.advance, y);
        window.paint_quad(gpui::fill(
            Bounds::from_corners(a, gpui::point(b.x, b.y + px(1.0))),
            t::metrics_line(),
        ));
    };
    let mut ys = vec![
        0.0,
        scene.box_top,
        scene.box_bottom,
        scene.upm,
        scene.ascender,
        scene.descender,
    ];
    ys.extend(scene.x_height);
    ys.extend(scene.cap_height);
    ys.retain(|y: &f64| y.is_finite());
    ys.sort_by(|a, b| a.total_cmp(b));
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.001);
    for y in ys {
        hline(y, window);
    }
}

/// Guides across the whole canvas: global then local, the hot one
/// brighter and thicker, each with a grab knob on its anchor.
fn paint_guides(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let bounds = s.bounds;
    let mut counts = (0_usize, 0_usize);
    for (local, line) in scene.guides.iter() {
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
        let hot = scene.guide_hot == Some((local, gi));
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
        // The knob sits on the guide's anchor: its stored point, or
        // the origin axis for plain H/V lines (UFO stores only the
        // offset).
        let knob = match *line {
            norad::Line::Horizontal(y) => {
                let p = s.to_screen(0.0, y);
                window.paint_quad(gpui::fill(
                    Bounds::from_corners(
                        gpui::point(bounds.origin.x, p.y),
                        gpui::point(bounds.origin.x + bounds.size.width, p.y + px(thick)),
                    ),
                    color,
                ));
                p
            }
            norad::Line::Vertical(x) => {
                let p = s.to_screen(x, 0.0);
                window.paint_quad(gpui::fill(
                    Bounds::from_corners(
                        gpui::point(p.x, bounds.origin.y),
                        gpui::point(p.x + px(thick), bounds.origin.y + bounds.size.height),
                    ),
                    color,
                ));
                p
            }
            norad::Line::Angle { x, y, degrees } => {
                // A segment far longer than any canvas; the editor
                // clips to its bounds.
                let (sin, cos) = degrees.to_radians().sin_cos();
                const R: f64 = 1.0e5;
                let a = s.to_screen(x - R * cos, y - R * sin);
                let b = s.to_screen(x + R * cos, y + R * sin);
                let mut pb = PathBuilder::stroke(px(thick));
                pb.move_to(a);
                pb.line_to(b);
                if let Ok(path) = pb.build() {
                    window.paint_path(path, color);
                }
                s.to_screen(x, y)
            }
        };
        // The grab knob, Glyphs-style.
        let r = if hot { 5.0 } else { 4.0 };
        let circle = {
            use kurbo::Shape as _;
            kurbo::Circle::new((f32::from(knob.x) as f64, f32::from(knob.y) as f64), r)
                .to_path(0.25)
        };
        if let Some(path) = build_fill_path(&circle, Affine::IDENTITY, zero()) {
            window.paint_path(path, color);
        }
    }
}

/// HOI knobs, one per node, and the live curve while one is dragged.
/// They ride on top of the trajectory tracks painted next.
fn paint_hoi_knobs(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if !scene.hoi_knobs.is_empty() {
        use kurbo::Shape as _;
        if let (Some((id, q)), Some((a, b))) = (scene.hoi_live, scene.hoi_drag_ends) {
            let _ = id;
            let mut pb = PathBuilder::stroke(px(1.5));
            for step in 0..=12 {
                let t = step as f64 / 12.0;
                let p = hoi_quad_at(a, b, q, t);
                let sp = s.to_screen(p.0, p.1);
                if step == 0 {
                    pb.move_to(sp);
                } else {
                    pb.line_to(sp);
                }
            }
            if let Ok(line) = pb.build() {
                window.paint_path(line, t::tool_feedback());
            }
        }
        for (id, q) in &scene.hoi_knobs {
            let dragging = scene.hoi_live.is_some_and(|(live, _)| live == *id);
            let q = if let Some((_, live)) = scene.hoi_live.filter(|(live, _)| live == id) {
                live
            } else {
                *q
            };
            let sp = s.to_screen(q.0, q.1);
            let dot = kurbo::Circle::new(
                (f32::from(sp.x) as f64, f32::from(sp.y) as f64),
                if dragging { 4.0 } else { 2.5 },
            )
            .to_path(0.25);
            if let Some(path) = build_fill_path(&dot, Affine::IDENTITY, zero()) {
                window.paint_path(
                    path,
                    if dragging {
                        t::tool_feedback()
                    } else {
                        t::text_muted()
                    },
                );
            }
        }
    }
}

/// HOI node trajectories: each point's path across the axis as a
/// thin line, under a velocity ribbon.
///
/// Dots sit at equal axis stops: close dots mean slow, spread dots
/// mean fast. Brace layers bend the line.
fn paint_trajectories(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if let Some(tracks) = &scene.trajectories {
        use kurbo::Shape as _;
        // The velocity ribbon (Glyphs' Show velocity): one block per
        // axis step, thickness and warmth scaling with how far the
        // node travels that step. Gold means the change rushes
        // there, ember means it lingers.
        for track in tracks {
            let steps: Vec<f64> = track.windows(2).map(|w| w[0].distance(w[1])).collect();
            let max_step = steps.iter().fold(0.0_f64, |a, &b| a.max(b));
            if max_step < 1.0 {
                continue; // static node
            }
            const RIBBON_PX: f32 = 13.0;
            for (i, w) in track.windows(2).enumerate() {
                let speed = steps[i] / max_step;
                let a = s.to_screen(w[0].x, w[0].y);
                let b = s.to_screen(w[1].x, w[1].y);
                let (ax, ay) = (f32::from(a.x), f32::from(a.y));
                let (bx, by) = (f32::from(b.x), f32::from(b.y));
                let (dx_, dy_) = (bx - ax, by - ay);
                let len = (dx_ * dx_ + dy_ * dy_).sqrt();
                if len < 0.5 {
                    continue;
                }
                // One-sided comb, like Glyphs': offset to the left
                // of travel.
                let (nx, ny) = (-dy_ / len, dx_ / len);
                let thick = RIBBON_PX * px32(speed);
                let mut quad = BezPath::new();
                quad.move_to((ax as f64, ay as f64));
                quad.line_to((bx as f64, by as f64));
                quad.line_to(((bx + nx * thick) as f64, (by + ny * thick) as f64));
                quad.line_to(((ax + nx * thick) as f64, (ay + ny * thick) as f64));
                quad.close_path();
                if let Some(path) = build_fill_path(&quad, Affine::IDENTITY, zero()) {
                    window.paint_path(path, t::velocity_ramp(speed));
                }
            }
        }
        for track in tracks {
            let mut pb = PathBuilder::stroke(px(1.0));
            for (i, p) in track.iter().enumerate() {
                let sp = s.to_screen(p.x, p.y);
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
                let sp = s.to_screen(p.x, p.y);
                let r = if i == 0 || i == last { 3.0 } else { 1.7 };
                let dot = kurbo::Circle::new((f32::from(sp.x) as f64, f32::from(sp.y) as f64), r)
                    .to_path(0.25);
                if let Some(path) = build_fill_path(&dot, Affine::IDENTITY, zero()) {
                    window.paint_path(path, t::trajectory_dot());
                }
            }
        }
    }
}

/// The two sidebearing edges, grown and recoloured under the pointer.
fn paint_sidebearings(scene: &EditorScene, s: &Screen, window: &mut Window) {
    for (right, x) in [(false, 0.0), (true, scene.advance)] {
        let hovered = scene.sidebearing_hover == Some(right);
        let a = s.to_screen(x, scene.box_top);
        let b = s.to_screen(x, scene.box_bottom);
        let (grow_l, grow_r) = if hovered { (1.0, 2.0) } else { (0.0, 1.0) };
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

/// Space-hold preview: the filled glyph and nothing else on top of
/// it. The masked preview is the truth the Bake Masks command makes
/// permanent.
fn paint_preview_fill(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if scene.preview_mode {
        let mut combined = scene.outline.as_ref().clone();
        combined.extend(scene.component_path.elements().iter().cloned());
        if !scene.mask_paths.is_empty() {
            let mut cut = BezPath::new();
            for m in &scene.mask_paths {
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
                    combined.extend(contour.path.elements().iter().copied());
                }
            }
        }
        if let Some(p) = build_fill_path(&combined, s.transform, s.origin) {
            window.paint_path(p, t::text());
        }
    }
}

/// The text buffer's quiet metric boxes and corner marks, before the
/// fills so marks sit under them.
fn paint_sort_boxes(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let sort_h_px = sort_height_px(scene, s.zoom);
    let mark = (sort_h_px * 0.05).clamp(1.5, 24.0);
    let marks_visible = mark >= 3.0;
    let (sort_top, sort_bottom, ascender) = (scene.sort_top, scene.sort_bottom, scene.ascender);
    if !scene.preview_mode && marks_visible {
        for sp in scene.sort_paints.iter() {
            // Quiet full box for the sorts nobody is editing (the
            // active one draws its own metrics outside text mode).
            if !sp.active {
                let color = t::metric_quiet();
                for ex in [sp.x, sp.x + sp.advance] {
                    paint_line(
                        s.to_screen(ex, sp.y + sort_bottom),
                        s.to_screen(ex, sp.y + sort_top),
                        color,
                        window,
                    );
                }
                for my in [sort_bottom, 0.0, ascender, sort_top] {
                    paint_line(
                        s.to_screen(sp.x, sp.y + my),
                        s.to_screen(sp.x + sp.advance, sp.y + my),
                        color,
                        window,
                    );
                }
            }
            // Corner marks: inward ticks at each metric height on
            // both edges, clipped to the box. Skipped for the active
            // sort outside text mode (it has the full green box
            // instead).
            if sp.active && !scene.text_mode {
                continue;
            }
            let color = match sp.kern {
                1 => t::kern_active(),
                2 => t::kern_previous(),
                _ => t::metrics_line(),
            };
            let ca = s.to_screen(sp.x, sp.y + sort_bottom);
            let cb = s.to_screen(sp.x + sp.advance, sp.y + sort_top);
            let (left, right) = (ca.x.min(cb.x), ca.x.max(cb.x));
            let (top_px, bottom_px) = (ca.y.min(cb.y), ca.y.max(cb.y));
            let mark_px = px(px32(mark));
            for ex in [sp.x, sp.x + sp.advance] {
                for my in [sort_bottom, 0.0, ascender, sort_top] {
                    let c = s.to_screen(ex, sp.y + my);
                    let x0 = (c.x - mark_px).max(left);
                    let x1 = (c.x + mark_px).min(right);
                    if x1 > x0 {
                        paint_line(gpui::point(x0, c.y), gpui::point(x1, c.y), color, window);
                    }
                    let y0 = (c.y - mark_px).max(top_px);
                    let y1 = (c.y + mark_px).min(bottom_px);
                    if y1 > y0 {
                        paint_line(gpui::point(c.x, y0), gpui::point(c.x, y1), color, window);
                    }
                }
            }
        }
    }
}

/// Sort fills.
///
/// Every sort but the active one fills. The active one fills too
/// while the text tool is up; its points return with select. Once the
/// design grid is up, you are drawing rather than reading, so the
/// neighbours thin to a 0.34 fill plus an outline with read-only grey
/// points. This is the web editor's zoomed-in treatment.
fn paint_sort_fills(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let (transform, origin) = (s.transform, s.origin);
    let zoomed_in = !scene.preview_mode && s.zoom > 0.8;
    let point_scale = point_scale(s.zoom);
    for sp in scene.sort_paints.iter() {
        // The active sort renders as editable chrome except in text
        // mode, where it is a plain fill like its neighbors. The
        // preview fill already drew it.
        if sp.active && (!scene.text_mode || scene.preview_mode) {
            continue;
        }
        let Some(path) = sp.path.as_ref() else {
            continue;
        };
        let dim = zoomed_in && !sp.active;
        let sort_transform = transform * Affine::translate((sp.x, sp.y));
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
        // Outline + read-only points so the neighbour reads as
        // structure.
        if let Some(p) = build_path(path, sort_transform, origin, PathBuilder::stroke(px(1.0))) {
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
        let hline2 = |a: kurbo::Point, b: kurbo::Point, pb: &mut PathBuilder, any: &mut bool| {
            pb.move_to(gpui::point(px(px32(a.x)), px(px32(a.y))));
            pb.line_to(gpui::point(px(px32(b.x)), px(px32(b.y))));
            *any = true;
        };
        for el in path.elements() {
            match *el {
                kurbo::PathEl::MoveTo(p) => {
                    let p = screen(p);
                    dots.extend(kurbo::Circle::new(p, on_r).to_path(0.25));
                    current = p;
                    start = p;
                }
                kurbo::PathEl::LineTo(p) => {
                    let p = screen(p);
                    dots.extend(kurbo::Circle::new(p, on_r).to_path(0.25));
                    current = p;
                }
                kurbo::PathEl::QuadTo(c, p) => {
                    let (c, p) = (screen(c), screen(p));
                    dots.extend(kurbo::Circle::new(c, off_r).to_path(0.25));
                    dots.extend(kurbo::Circle::new(p, on_r).to_path(0.25));
                    hline2(current, c, &mut handles, &mut any_handles);
                    hline2(c, p, &mut handles, &mut any_handles);
                    current = p;
                }
                kurbo::PathEl::CurveTo(c1, c2, p) => {
                    let (c1, c2, p) = (screen(c1), screen(c2), screen(p));
                    dots.extend(kurbo::Circle::new(c1, off_r).to_path(0.25));
                    dots.extend(kurbo::Circle::new(c2, off_r).to_path(0.25));
                    dots.extend(kurbo::Circle::new(p, on_r).to_path(0.25));
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
        if let Some(p) = build_fill_path(&dots, Affine::IDENTITY, zero()) {
            window.paint_path(p, t::point_inner());
        }
        if let Some(p) = build_path(
            &dots,
            Affine::IDENTITY,
            zero(),
            PathBuilder::stroke(px(1.0)),
        ) {
            window.paint_path(p, t::point_readonly());
        }
    }
}

/// Caret: a line plus inward triangles, sized off the sort's
/// on-screen height. The web editor sizes its caret the same way.
fn paint_text_caret(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if let Some((cx_, cy)) = scene.text_caret {
        let sort_h_px = sort_height_px(scene, s.zoom);
        let top = s.to_screen(cx_, cy + scene.sort_top);
        let bottom = s.to_screen(cx_, cy + scene.sort_bottom);
        let caret_color = t::text_cursor();
        window.paint_quad(gpui::fill(
            Bounds::from_corners(
                gpui::point(top.x - px(0.75), top.y),
                gpui::point(top.x + px(0.75), bottom.y),
            ),
            caret_color,
        ));
        let tri_scale = ((sort_h_px * 0.09).clamp(4.0, 34.0)) / 24.0;
        let tw = px(px32(24.0 * tri_scale));
        let th = px(px32(16.0 * tri_scale));
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
}

/// Reference layers: other masters as dim strokes.
fn paint_reference_layers(scene: &EditorScene, s: &Screen, window: &mut Window) {
    for path in &scene.reference_paths {
        if let Some(p) = build_path(path, s.transform, s.origin, PathBuilder::stroke(px(1.0))) {
            window.paint_path(p, t::reference_layer());
        }
    }
}

/// Components: a dim distinct fill, not editable directly.
/// Cmd+Shift+D decomposes.
fn paint_components(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if !scene.component_path.elements().is_empty()
        && let Some(p) = build_fill_path(&scene.component_path, s.transform, s.origin)
    {
        let color = if scene.component_selected {
            t::component_selected_fill()
        } else {
            t::component_fill()
        };
        window.paint_path(p, color);
    }
}

/// The quiet layers behind the drawing: the interpolated ghost, the
/// reference glyph, the background layer, and the per-glyph layers
/// with the eye on.
fn paint_background_layers(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let (transform, origin) = (s.transform, s.origin);
    // Interpolated instance at the axes-bar location, as a ghost
    // outline.
    if let Some(ghost) = &scene.ghost
        && let Some(p) = build_path(ghost, transform, origin, PathBuilder::stroke(px(1.0)))
    {
        window.paint_path(p, t::ghost());
    }
    // Reference glyph: a ghost fill so it never reads as the
    // background layer's outline.
    if let Some(path) = &scene.reference_path
        && let Some(p) = build_fill_path(path, transform, origin)
    {
        let mut fill = t::glyph_fill();
        fill.a *= 0.22;
        window.paint_path(p, fill);
    }
    // Background layer: a quiet outline behind the drawing, the way
    // Glyphs shows a background.
    if let Some(path) = &scene.background_path
        && let Some(p) = build_path(path, transform, origin, PathBuilder::stroke(px(1.0)))
    {
        window.paint_path(p, t::metric_quiet());
    }
    // Per-glyph layers with the eye on: same quiet outline as the
    // background.
    for path in &scene.glyph_layer_paths {
        if let Some(p) = build_path(path, transform, origin, PathBuilder::stroke(px(1.0))) {
            window.paint_path(p, t::metric_quiet());
        }
    }
}

/// The mark cloud, faint fills.
fn paint_mark_cloud(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if !scene.mark_cloud.is_empty() {
        let mut ghost = t::glyph_fill();
        ghost.a *= 0.10;
        for path in &scene.mark_cloud {
            if let Some(p) = build_fill_path(path, s.transform, s.origin) {
                window.paint_path(p, ghost);
            }
        }
    }
}

/// Mask contours read as cuts: the local-guide accent over the normal
/// stroke.
fn paint_masks(scene: &EditorScene, s: &Screen, window: &mut Window) {
    for path in &scene.mask_paths {
        if let Some(p) = build_path(path, s.transform, s.origin, PathBuilder::stroke(px(2.0))) {
            window.paint_path(p, t::guide_local());
        }
    }
}

/// Curvature comb, behind the outline so points stay selectable over
/// it.
fn paint_curvature_comb(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let transform = s.transform;
    for strip in &scene.comb_strips {
        for w in strip.windows(2) {
            let (s0, s1) = (&w[0], &w[1]);
            let mut quad = BezPath::new();
            quad.move_to(transform * s0.on);
            quad.line_to(transform * s1.on);
            quad.line_to(transform * s1.outer);
            quad.line_to(transform * s0.outer);
            quad.close_path();
            let k = if scene.comb_maxk > 1e-12 {
                (s0.kappa.abs() + s1.kappa.abs()) * 0.5 / scene.comb_maxk
            } else {
                0.0
            };
            if let Some(p) = build_fill_path(&quad, Affine::IDENTITY, s.origin) {
                window.paint_path(p, t::comb_gradient(k));
            }
        }
    }
}

/// The glyph being edited: a ghost fill under a stroked outline.
///
/// The ghost fill is the inactive sorts' grey at a tenth strength, so
/// counters read as counters without competing with the outline. The
/// outline itself is stroked with no fill, like the other editors.
/// The fill alpha is the web editor's `ACTIVE_GLYPH_FILL_ALPHA`.
fn paint_outline(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let (transform, origin) = (s.transform, s.origin);
    if !scene.preview_mode && !scene.text_mode {
        let mut combined = scene.outline.as_ref().clone();
        combined.extend(scene.component_path.elements().iter().cloned());
        if let Some(p) = build_fill_path(&combined, transform, origin) {
            window.paint_path(p, t::outline_fill());
        }
    }
    if !scene.preview_mode
        && !scene.text_mode
        && let Some(path) = build_path(
            &scene.outline,
            transform,
            origin,
            PathBuilder::stroke(px(1.0)),
        )
    {
        window.paint_path(path, t::path_stroke());
    }
}

/// Handle lines: each off-curve connects to its anchoring on-curve
/// neighbor.
fn paint_handles(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if !scene.preview_mode && !scene.text_mode {
        let points = &scene.points;
        let mut lines = PathBuilder::stroke(px(1.0));
        let mut any_line = false;
        for (i, p) in points.iter().enumerate() {
            if p.on_curve {
                continue;
            }
            // Neighbors within the same contour, cyclic.
            let contour_pts: Vec<&GlyphPoint> =
                points.iter().filter(|q| q.contour == p.contour).collect();
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
            lines.move_to(s.to_screen(p.x, p.y));
            lines.line_to(s.to_screen(anchor.x, anchor.y));
            any_line = true;
            let _ = i;
        }
        if any_line && let Ok(path) = lines.build() {
            window.paint_path(path, t::handle_line());
        }
    }
}

/// Points: smooth = blue circle, corner = green square, off-curve =
/// purple circle, selection in yellow/orange, the shared palette.
///
/// A point is a dark window with a coloured ring, the web editor's
/// recipe. A halo casing keeps an edge over the outline and the comb.
/// An interior fill masks what runs underneath. A constant-width ring
/// sits on top. Selected points fill yellow and ring in the selection
/// colour. Three path draws for every point on the glyph, plus the
/// gridlines, collapse into one per colour.
fn paint_points(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let transform = s.transform;
    let (grid_mid_alpha, grid_close_alpha) = grid_alphas(s.zoom);
    let (ps, ring_w, halo_w) = point_widths(s.zoom);
    let shape = |center: Point<gpui::Pixels>, r: f32, square: bool| -> BezPath {
        use kurbo::Shape as _;
        let (cx_, cy_) = (f32::from(center.x) as f64, f32::from(center.y) as f64);
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
    let zero = zero();
    let mut halo_batch: Vec<BezPath> = Vec::new();
    let mut fill_batch: ColorBatch = std::collections::BTreeMap::new();
    let mut ring_batch: ColorBatch = std::collections::BTreeMap::new();
    let mut chord_batch: std::collections::BTreeMap<u32, (gpui::Rgba, Vec<(f32, BezPath)>)> =
        std::collections::BTreeMap::new();
    for p in scene.points.iter() {
        if scene.preview_mode || scene.text_mode {
            break;
        }
        let center = s.to_screen(p.x, p.y);
        let is_selected = scene.selected_points.contains(&(p.contour, p.index));
        let is_locked = scene.locked_points.contains(&(p.contour, p.index));
        let hue = if p.hyper {
            t::point_hyper_outer()
        } else if !p.on_curve {
            t::point_offcurve_outer()
        } else if p.smooth {
            t::point_smooth_outer()
        } else {
            t::point_corner_outer()
        };
        // The hue is the ring or the fill, by the theme's recipe; the
        // grid chords inside the point take the hue either way.
        let (ring, inner) = if is_locked {
            // Locked nodes read as inert.
            (t::point_readonly(), t::point_readonly())
        } else if is_selected {
            (t::point_selected_ring(), t::point_selected())
        } else if t::points_filled() {
            (t::point_outline(), hue)
        } else {
            (hue, t::point_inner())
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
        // The point is a window onto the design grid: the grid that
        // falls inside it is redrawn on top, tinted with the point's
        // own colour, so you can read where it sits. Dots when the
        // grid is dots; chords of the gridlines when it is lines
        // (web clips the grid to the point; gpui masks rectangles
        // only, so the chords are solved instead).
        if grid_mid_alpha > 0.0 && !scene.preview_mode && !scene.text_mode {
            let (cx_, cy_) = (f32::from(center.x) as f64, f32::from(center.y) as f64);
            let r = r as f64;
            let inv = transform.inverse();
            let a = (inv * kurbo::Point::new(cx_ - r, cy_)).x;
            let b = (inv * kurbo::Point::new(cx_ + r, cy_)).x;
            let (lo_x, hi_x) = (a.min(b), a.max(b));
            let a = (inv * kurbo::Point::new(cx_, cy_ - r)).y;
            let b = (inv * kurbo::Point::new(cx_, cy_ + r)).y;
            let (lo_y, hi_y) = (a.min(b), a.max(b));
            for (spacing, alpha, size, wide) in [
                (8.0_f64, grid_mid_alpha, 1.5_f32, 1.0_f32),
                (2.0, grid_close_alpha, 1.0, 0.7),
            ] {
                if alpha <= 0.0 {
                    continue;
                }
                let mut tint = if is_locked || is_selected { ring } else { hue };
                tint.a = px32(alpha);
                let ks = to_index((lo_x / spacing).ceil())..=to_index((hi_x / spacing).floor());
                let ls = to_index((lo_y / spacing).ceil())..=to_index((hi_y / spacing).floor());
                let mut marks = BezPath::new();
                if scene.grid_lines {
                    // The chord is the circle's half-height at that
                    // offset, the full radius for a square point.
                    let half_at = |d: f64| {
                        if is_square {
                            r
                        } else {
                            (r * r - d * d).max(0.0).sqrt()
                        }
                    };
                    for k in ks.clone() {
                        let sx = (transform * kurbo::Point::new(k as f64 * spacing, 0.0)).x;
                        let half = half_at(sx - cx_);
                        if half > 0.2 {
                            marks.move_to((sx, cy_ - half));
                            marks.line_to((sx, cy_ + half));
                        }
                    }
                    for l in ls.clone() {
                        let sy = (transform * kurbo::Point::new(0.0, l as f64 * spacing)).y;
                        let half = half_at(sy - cy_);
                        if half > 0.2 {
                            marks.move_to((cx_ - half, sy));
                            marks.line_to((cx_ + half, sy));
                        }
                    }
                } else {
                    let h = (size / 2.0) as f64;
                    for k in ks.clone() {
                        for l in ls.clone() {
                            let at = transform
                                * kurbo::Point::new(k as f64 * spacing, l as f64 * spacing);
                            let (dx, dy) = (at.x - cx_, at.y - cy_);
                            let inside = if is_square {
                                dx.abs() <= r && dy.abs() <= r
                            } else {
                                dx * dx + dy * dy <= r * r
                            };
                            if inside {
                                marks.extend(kurbo::Shape::to_path(
                                    &kurbo::Rect::new(at.x - h, at.y - h, at.x + h, at.y + h),
                                    0.1,
                                ));
                            }
                        }
                    }
                }
                if !marks.is_empty() {
                    let width = if scene.grid_lines { wide } else { 0.0 };
                    let entry = chord_batch
                        .entry(color_key(tint))
                        .or_insert_with(|| (tint, Vec::new()));
                    match entry.1.iter_mut().find(|(w, _)| *w == width) {
                        Some((_, acc)) => acc.extend(marks.iter()),
                        None => entry.1.push((width, marks)),
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
    if t::point_halo() {
        paint_batched(window, zero, t::halo(), &halo_batch, Some(halo_w));
    }
    for (color, paths) in fill_batch.values() {
        paint_batched(window, zero, *color, paths, None);
    }
    for (color, path) in chord_batch.values() {
        for (width, path) in path {
            // Width zero marks a filled dot; otherwise a stroked chord.
            let built = if *width > 0.0 {
                build_path(
                    path,
                    Affine::IDENTITY,
                    zero,
                    PathBuilder::stroke(px(*width)),
                )
            } else {
                build_fill_path(path, Affine::IDENTITY, zero)
            };
            if let Some(p) = built {
                window.paint_path(p, *color);
            }
        }
    }
    for (color, paths) in ring_batch.values() {
        paint_batched(window, zero, *color, paths, Some(ring_w));
    }
}

/// Start-of-contour arrow: which point a closed contour begins at,
/// and which way it runs. This is the web editor's
/// `draw_start_arrow`.
fn paint_start_markers(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let (ps, _, _) = point_widths(s.zoom);
    if !scene.preview_mode && !scene.text_mode {
        for start in scene.start_markers.iter() {
            let (from, to, selected) = *start;
            let a = s.to_screen(from.0, from.1);
            let b = s.to_screen(to.0, to.1);
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
            let left = (base.0 + perp.0 * size * 0.5, base.1 + perp.1 * size * 0.5);
            let right = (base.0 - perp.0 * size * 0.5, base.1 - perp.1 * size * 0.5);
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
}

/// Anchors are diamonds built like points: a dark window with a
/// coloured ring.
///
/// The diamond is sized off the smooth-point radius and widened a
/// little, so a rotated square reads as the same size. The widening
/// is the web editor's `ANCHOR_DIAMOND_SCALE`.
fn paint_anchors(scene: &EditorScene, s: &Screen, window: &mut Window) {
    let (ps, ring_w, halo_w) = point_widths(s.zoom);
    let zero = zero();
    let mut anchor_halo: Vec<BezPath> = Vec::new();
    let mut anchor_fill: ColorBatch = std::collections::BTreeMap::new();
    let mut anchor_ring: ColorBatch = std::collections::BTreeMap::new();
    for (ai, (_, ax, ay)) in scene.anchors.iter().enumerate() {
        if scene.preview_mode || scene.text_mode {
            break;
        }
        let center = s.to_screen(*ax, *ay);
        let is_selected = scene.selected_anchors.contains(&ai);
        let r = (if is_selected { 5.5 } else { 4.5 }) * ps * 1.35;
        let (cx_, cy_) = (f32::from(center.x) as f64, f32::from(center.y) as f64);
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
    if t::point_halo() {
        paint_batched(window, zero, t::halo(), &anchor_halo, Some(halo_w));
    }
    for (color, paths) in anchor_fill.values() {
        paint_batched(window, zero, *color, paths, None);
    }
    for (color, paths) in anchor_ring.values() {
        paint_batched(window, zero, *color, paths, Some(ring_w));
    }
}

/// The live previews of the tools: the shapes-tool rectangle or
/// ellipse, the alt-hovered segment, the pen rubber band, the knife
/// line with its hits, and the measure-tool line.
fn paint_tool_preview(scene: &EditorScene, s: &Screen, window: &mut Window) {
    // Shapes-tool live preview.
    if let Some((a, b, ellipse)) = scene.shape_preview {
        use kurbo::Shape as _;
        let rect =
            kurbo::Rect::from_points(kurbo::Point::new(a.0, a.1), kurbo::Point::new(b.0, b.1));
        let shape: BezPath = if ellipse {
            kurbo::Ellipse::from_rect(rect).to_path(0.1)
        } else {
            rect.to_path(0.1)
        };
        if let Some(p) = build_path(&shape, s.transform, s.origin, PathBuilder::stroke(px(1.0))) {
            window.paint_path(p, t::tool_feedback());
        }
    }
    // Measure-tool line.
    if let Some(seg) = scene.hover_seg {
        let mut pb = PathBuilder::stroke(px(3.0));
        match seg {
            kurbo::PathSeg::Line(l) => {
                pb.move_to(s.to_screen(l.p0.x, l.p0.y));
                pb.line_to(s.to_screen(l.p1.x, l.p1.y));
            }
            kurbo::PathSeg::Quad(q) => {
                pb.move_to(s.to_screen(q.p0.x, q.p0.y));
                pb.curve_to(s.to_screen(q.p2.x, q.p2.y), s.to_screen(q.p1.x, q.p1.y));
            }
            kurbo::PathSeg::Cubic(c) => {
                pb.move_to(s.to_screen(c.p0.x, c.p0.y));
                pb.cubic_bezier_to(
                    s.to_screen(c.p3.x, c.p3.y),
                    s.to_screen(c.p1.x, c.p1.y),
                    s.to_screen(c.p2.x, c.p2.y),
                );
            }
        }
        if let Ok(p) = pb.build() {
            window.paint_path(p, t::tool_feedback());
        }
    }
    if let Some(((lx, ly), (cx3, cy3), close)) = scene.pen_preview {
        let mut pb = PathBuilder::stroke(px(1.0));
        pb.move_to(s.to_screen(lx, ly));
        pb.line_to(s.to_screen(cx3, cy3));
        if let Ok(p) = pb.build() {
            window.paint_path(p, t::tool_feedback());
        }
        if let Some((sx2, sy2)) = close {
            paint_circle(window, s.to_screen(sx2, sy2), 6.0, t::tool_feedback());
        }
    }
    if let Some(((sx, sy), (cx2, cy2), hits)) = &scene.knife_line {
        let a = s.to_screen(*sx, *sy);
        let b = s.to_screen(*cx2, *cy2);
        let mut line = PathBuilder::stroke(px(1.0));
        line.move_to(a);
        line.line_to(b);
        if let Ok(p) = line.build() {
            window.paint_path(p, t::anchor());
        }
        for hit in hits {
            let c = s.to_screen(hit.x, hit.y);
            paint_circle(window, c, 3.5, t::anchor());
        }
    }
    if let Some((a, b)) = scene.measure_line {
        let mut pb = PathBuilder::stroke(px(1.0));
        let pa = s.to_screen(a.0, a.1);
        let pbp = s.to_screen(b.0, b.1);
        pb.move_to(pa);
        pb.line_to(pbp);
        if let Ok(p) = pb.build() {
            window.paint_path(p, t::tool_feedback());
        }
    }
}

/// The measure-tool HUD: popcount-colored outline, dimension lines
/// with outward arrowheads, and labels that dodge each other.
///
/// Fades in with zoom. This is the web editor's `draw_measurements`.
fn paint_measure_hud(scene: &EditorScene, s: &Screen, window: &mut Window, cx: &mut App) {
    let (transform, origin, zoom) = (s.transform, s.origin, s.zoom);
    let measure_opts = scene.measure_opts;
    if let Some((strokes, measurements, sb)) = &scene.measure_hud {
        use runebender_core::analysis::measure::{self, MeasureKind};
        let t32 = px32(((zoom - 0.30) / 0.40).clamp(0.0, 1.0));
        if t32 > 0.0 {
            let fade = |mut c: gpui::Rgba, mul: f32| {
                c.a *= t32 * mul;
                c
            };
            for cs in strokes {
                let width = if cs.wide { 1.5 } else { 1.0 };
                if let Some(p) =
                    build_path(&cs.path, transform, origin, PathBuilder::stroke(px(width)))
                {
                    window.paint_path(p, fade(t::popcount_tier(cs.popcount), 1.0));
                }
            }
            let gp =
                |p: kurbo::Point| gpui::point(origin.x + px(px32(p.x)), origin.y + px(px32(p.y)));
            // A span's dimension line: a shaft that stops short of
            // both endpoints with an outward arrowhead at each end.
            let dim_line =
                |window: &mut Window, a: kurbo::Point, b: kurbo::Point, color: gpui::Rgba| {
                    let (dx, dy) = (b.x - a.x, b.y - a.y);
                    let len = dx.hypot(dy);
                    if len < 1e-3 {
                        return;
                    }
                    let (ux, uy) = (dx / len, dy / len);
                    let (nx, ny) = (-uy, ux);
                    let (end_gap, head, wing) = (3.0, 7.0, 4.0);
                    let a2 = kurbo::Point::new(a.x + ux * end_gap, a.y + uy * end_gap);
                    let b2 = kurbo::Point::new(b.x - ux * end_gap, b.y - uy * end_gap);
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
            // Place a label just off its line, then step it outward
            // (and to the other side) until it clears every label
            // already placed this frame.
            let label_px = px(crate::workspace::UI_TEXT_PX);
            let line_h = px((crate::workspace::UI_TEXT_PX * 1.15).ceil());
            let label_font = window.text_style().font();
            let mut placed: Vec<kurbo::Rect> = Vec::new();
            let draw_label = |window: &mut Window,
                              cx: &mut App,
                              a: kurbo::Point,
                              b: kurbo::Point,
                              text: String,
                              color: gpui::Rgba,
                              placed: &mut Vec<kurbo::Rect>| {
                let label_text = gpui::SharedString::from(text);
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
                if (horizontalish && ny > 0.0) || (!horizontalish && nx < 0.0) {
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
                        let cand = top_left(dirx, diry, base + k as f64 * step);
                        let rect = kurbo::Rect::new(
                            cand.x - pad,
                            cand.y - pad,
                            cand.x + w + pad,
                            cand.y + h + pad,
                        );
                        let clear = !placed.iter().any(|r| {
                            r.x0 < rect.x1 && rect.x0 < r.x1 && r.y0 < rect.y1 && rect.y0 < r.y1
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
                // A casing around the numerals, not a filled box: the
                // web strokes each glyph in the halo colour before
                // filling it. gpui has no stroked text, so the line
                // is painted eight times around the centre instead,
                // which reads the same and keeps the canvas visible
                // behind the label.
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
                let halo_line = window.text_system().shape_line(
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
                        gp(kurbo::Point::new(chosen.x + ox, chosen.y + oy)),
                        line_h,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
                let _ = line.paint(gp(chosen), line_h, gpui::TextAlign::Left, None, window, cx);
            };
            if let Some(sb) = sb {
                for (is_left, x, y, val) in [
                    (true, sb.min_x, sb.y_left, sb.lsb),
                    (false, sb.max_x, sb.y_right, sb.rsb),
                ] {
                    let color = fade(t::popcount_tier(measure::popcount(val)), 0.9);
                    let margin_x = if is_left { 0.0 } else { sb.advance };
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
                    MeasureKind::Horizontal | MeasureKind::Vertical => measure_opts.spans,
                };
                if !show {
                    continue;
                }
                let a = transform * m.a;
                let b = transform * m.b;
                let color = fade(t::popcount_tier(measure::popcount(m.length)), 1.0);
                if matches!(m.kind, MeasureKind::Horizontal | MeasureKind::Vertical) {
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
            // Segment sizes: each curve's own box, labelled at its
            // centre, so the whole glyph can be read at once instead
            // of one selection at a time.
            for b in scene.segment_boxes.iter() {
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
                frame.move_to(gp(kurbo::Point::new(corners[0].0, corners[0].1)));
                for (x, y) in corners.iter().skip(1) {
                    frame.line_to(gp(kurbo::Point::new(*x, *y)));
                }
                frame.line_to(gp(kurbo::Point::new(corners[0].0, corners[0].1)));
                let color = fade(t::metric_quiet(), 1.0);
                if let Ok(p) = frame.build() {
                    window.paint_path(p, color);
                }
                let text = format!("{:.0}×{:.0}", b.width(), b.height());
                let mid_left = kurbo::Point::new(rect.x0, rect.center().y);
                let mid_right = kurbo::Point::new(rect.x1, rect.center().y);
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
}

/// Continuity rings around on-curve nodes.
fn paint_continuity_rings(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if !scene.continuity_rings.is_empty() {
        use kurbo::Shape as _;
        let r = 4.5 * 1.9;
        for (at, color) in &scene.continuity_rings {
            let c = s.transform * *at;
            let circle = kurbo::Circle::new(c, r).to_path(0.25);
            if let Some(p) = build_path(
                &circle,
                Affine::IDENTITY,
                s.origin,
                PathBuilder::stroke(px(1.5)),
            ) {
                window.paint_path(p, *color);
            }
        }
    }
}

/// Annotations: red working marks over everything. Arrows point at
/// the spot, circles ring it, notes label it.
fn paint_annotations(scene: &EditorScene, s: &Screen, window: &mut Window, cx: &mut App) {
    if !scene.annotations.is_empty() {
        use kurbo::Shape as _;
        let color = t::annotation();
        for note in &scene.annotations {
            let p = s.to_screen(note.x, note.y);
            let (px_, py_) = (f32::from(p.x) as f64, f32::from(p.y) as f64);
            match note.kind.as_str() {
                "circle" => {
                    let ring = kurbo::Circle::new((px_, py_), 12.0).to_path(0.25);
                    if let Some(path) = build_path(
                        &ring,
                        Affine::IDENTITY,
                        zero(),
                        PathBuilder::stroke(px(2.0)),
                    ) {
                        window.paint_path(path, color);
                    }
                }
                "note" => {
                    let dot = kurbo::Circle::new((px_, py_), 3.0).to_path(0.25);
                    if let Some(path) = build_fill_path(&dot, Affine::IDENTITY, zero()) {
                        window.paint_path(path, color);
                    }
                    let text = gpui::SharedString::from(note.text.clone());
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
                    // Arrow from lower-right, tip on the point.
                    let mut arrow = BezPath::new();
                    arrow.move_to((px_, py_));
                    arrow.line_to((px_ + 12.0, py_ + 4.0));
                    arrow.line_to((px_ + 8.0, py_ + 8.0));
                    arrow.line_to((px_ + 20.0, py_ + 20.0));
                    arrow.line_to((px_ + 8.0 + 4.0, py_ + 8.0 + 8.0));
                    arrow.line_to((px_ + 4.0, py_ + 12.0));
                    arrow.close_path();
                    if let Some(path) = build_fill_path(&arrow, Affine::IDENTITY, zero()) {
                        window.paint_path(path, color);
                    }
                }
            }
        }
    }
}

/// Free-transform box: the selection's bounds with corner and edge
/// handles, all constant screen size. This is the on-canvas rotate
/// and scale in Glyphs 4.
fn paint_transform_box(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if let Some(bbox) = scene.transform_box {
        let pa = s.to_screen(bbox.x0, bbox.y0);
        let pb = s.to_screen(bbox.x1, bbox.y1);
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
            let p = s.to_screen(hx, hy);
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
}

/// Marquee rectangle.
fn paint_marquee(scene: &EditorScene, s: &Screen, window: &mut Window) {
    if let Some((a, b)) = scene.marquee {
        let pa = s.to_screen(a.0, a.1);
        let pb = s.to_screen(b.0, b.1);
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
}
