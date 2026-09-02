// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The editor's state: the `Workspace` struct and the types it is made of.
//!
//! Everything the window shows comes from here. The methods live in
//! `view/`, `edit/`, and `platform/`, one file per concern, and
//! `wiring.rs` builds the first value.

use crate::Arc;
use crate::Mutex;
use crate::PathBuf;
#[cfg(not(target_family = "wasm"))]
use crate::platform::config;
#[cfg(target_family = "wasm")]
#[cfg(target_family = "wasm")]
#[cfg(target_family = "wasm")]
#[cfg(target_family = "wasm")]
#[cfg(target_family = "wasm")]
use crate::platform::web_host;
use crate::view::grid::OrderKey;
use crate::widgets;
use gpui::Bounds;
use gpui::Point;
use gpui::SharedString;
use kurbo::Affine;
use runebender_core::analysis::search::SearchPred;
use runebender_core::document::project::Project;
use runebender_core::ui::editing::ViewPort;
use std::collections::{HashMap, HashSet};

/// How the font overview presents the glyph set.
///
/// Grid, detail, and list are Font View's three modes in Glyphs 4.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum FontViewMode {
    /// The classic grid of glyph cells.
    Grid,
    /// The detail grid: info columns beside every cell.
    Detail,
    /// The property table: one row per glyph.
    List,
    /// The positional-forms matrix for Arabic review: isol, init,
    /// medi, and fina as columns per base letter.
    Matrix,
}

/// Built-in sample strings: spacing control strings and kern words.
/// View > Next Sample String cycles them around the open glyph.
pub(crate) const SAMPLE_STRINGS: &[&str] = &[
    "HHOHOHOO",
    "nnonoonoo",
    "hamburgefonstiv",
    "HAMBURGEFONSTIV",
    "0123456789",
    "AVATAR Wave Toy Vy",
    "((\"quoted\")) [j] {f}!?",
];

/// Which metric field is being edited.
#[derive(Clone, Copy)]
pub(crate) enum MetricField {
    /// The advance width field.
    Width,
    /// The left sidebearing field.
    Lsb,
    /// The right sidebearing field.
    Rsb,
}

/// The active editor tool.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    /// Pointer tool: selects, moves, and transforms points.
    Select,
    /// Bezier pen: places on-curve points, drags out handles.
    Pen,
    /// Drags out rectangles or ellipses.
    Shapes,
    /// Types glyphs into the editor's text buffer.
    Text,
    /// Cuts contours along a dragged line.
    Knife,
    /// Filled preview with the editing chrome hidden.
    Preview,
    /// Pen that draws hyperbezier contours.
    HyperPen,
    /// Measures distances and shows the measurement HUD layers.
    Measure,
}

/// Pen-tool drawing state: the open contour and the outgoing handle
/// of the previously placed point (set by click-dragging it).
pub(crate) struct PenState {
    /// Index of the open contour being drawn.
    pub(crate) contour: usize,
    /// The previous point's outgoing handle, if it was dragged out.
    pub(crate) prev_out_handle: Option<(f64, f64)>,
    /// While the mouse is down on a fresh point: its position, used
    /// to mirror the dragged handle.
    pub(crate) placing: Option<(f64, f64)>,
}

/// An in-progress mouse gesture on the editor canvas.
pub(crate) enum Drag {
    /// Moving the selected points: gesture start in design space and
    /// every point's position when the gesture began. Handles that
    /// travel with a selected on-curve point need their start
    /// positions too, so this covers the whole glyph rather than just
    /// the selection.
    Points {
        /// Where the gesture began, in design space.
        start: (f64, f64),
        /// Every point's position when the gesture began, keyed by
        /// `(contour, point)`.
        originals: HashMap<(usize, usize), (f64, f64)>,
        /// Index and start position of each selected anchor.
        /// Anchors travel with the points on the same delta.
        anchor: Vec<(usize, (f64, f64))>,
    },
    /// Manual kerning drag in the text buffer (engine session).
    TextKern,
    /// Panning the viewport by alt-drag with the select tool.
    /// The anchor is in window space.
    Pan {
        /// The last pointer position, in window space.
        last: (f64, f64),
    },
    /// Dragging a sidebearing edge (false = left, true = right).
    Sidebearing {
        /// True when the right edge is dragged, false the left.
        right: bool,
        /// Pointer x where the drag began, in design space.
        start_x: f64,
        /// The delta already applied, so each move adds only the
        /// difference.
        applied: f64,
        /// The glyph's advance width when the drag began.
        start_width: f64,
    },
    /// Free transform from the selection bounding box: a handle
    /// scales about the opposite handle, the ring outside a corner
    /// rotates about the box centre. Shift constrains: proportional
    /// scale, 15-degree rotation steps.
    FreeTransform {
        /// The fixed point of the gesture, in design space.
        anchor: (f64, f64),
        /// Where the gesture began, in design space.
        start: (f64, f64),
        /// Rotation instead of scaling.
        rotate: bool,
        /// Which axes a scale handle drives (edge handles pin one).
        scale_x: bool,
        /// The same, for the y axis.
        scale_y: bool,
        /// Every point's position when the gesture began.
        originals: HashMap<(usize, usize), (f64, f64)>,
    },
    /// Dragging a node's HOI intermediate knob: the point id and
    /// the node's positions in the axis-end masters.
    HoiKnob {
        /// The dragged point, as `(contour, point)` indices.
        id: (usize, usize),
        /// The node's position in the low axis-end master.
        a: (f64, f64),
        /// The node's position in the high axis-end master.
        b: (f64, f64),
    },
    /// Dragging a guide: `local` picks the open glyph's guidelines
    /// over the master's fontinfo ones. Guides move live; the
    /// master is marked dirty as they move.
    Guide {
        /// True picks the open glyph's guidelines, false the master's
        /// fontinfo ones.
        local: bool,
        /// Index into the chosen guideline list.
        index: usize,
    },
    /// Dragging the selected component.
    Component {
        /// Index of the dragged component in the glyph.
        index: usize,
        /// Where the gesture began, in design space.
        start: (f64, f64),
        /// The component's offset when the gesture began.
        orig: (f64, f64),
    },
    /// Knife line, in design space.
    Knife {
        /// Where the cut line begins, in design space.
        start: (f64, f64),
        /// The pointer's current position.
        current: (f64, f64),
    },
    /// Rubber-band selection rectangle, in design space. `base` is
    /// what was selected when the drag began: the live selection is
    /// always that plus whatever the box now encloses.
    Marquee {
        /// The corner where the drag began.
        start: (f64, f64),
        /// The pointer's current position.
        current: (f64, f64),
        /// Points selected when the drag began.
        base: HashSet<(usize, usize)>,
        /// Anchors selected when the drag began.
        base_anchors: Vec<usize>,
    },
    /// Dragging out a rectangle/ellipse (shapes tool).
    Shape {
        /// The corner where the drag began.
        start: (f64, f64),
        /// The pointer's current position.
        current: (f64, f64),
    },
    /// Measuring (measure tool).
    Measure {
        /// Where the measuring line begins.
        start: (f64, f64),
        /// The pointer's current position.
        current: (f64, f64),
    },
}

/// A right-click context menu over the editor canvas.
///
/// This is `contourContextMenu` in the web editor.
pub(crate) struct ContextMenu {
    /// Position inside the canvas, in canvas-local pixels.
    pub(crate) at: Point<gpui::Pixels>,
    /// The click position, in design space.
    pub(crate) design: (f64, f64),
    /// The contour under the click, if any.
    pub(crate) contour: Option<usize>,
    /// How many contours the glyph has.
    pub(crate) contour_count: usize,
    /// The nearest on-curve point (for Set Start Point), as
    /// `(contour, point)`.
    pub(crate) start_point: Option<(usize, usize)>,
    /// The anchor under the click, if any.
    pub(crate) anchor: Option<usize>,
    /// The component under the click: its index and whether it is
    /// aligned.
    pub(crate) component: Option<(usize, bool)>,
    /// The glyph has at least one component.
    pub(crate) has_components: bool,
    /// Inline component-name input mode (Add Component).
    pub(crate) adding_component: bool,
    /// Inline corner-name input mode (Apply Corner…).
    pub(crate) applying_corner: bool,
    /// Inline note-text input mode (Annotate: Note…).
    pub(crate) adding_note: bool,
    /// Annotation under the click.
    pub(crate) annotation: Option<usize>,
    /// Guide under the click: (local, index).
    pub(crate) guide: Option<(bool, usize)>,
}

/// The edit view's live state: viewport, tool, selection, undo,
/// and the drag in progress.
pub(crate) struct EditorState {
    /// The active text-buffer sort's layout position (design units);
    /// (0,0) when the glyph is alone in the editor.
    pub(crate) sort_offset: (f64, f64),
    /// The tool to return to when space-hold preview ends.
    pub(crate) previous_tool: Tool,
    /// The hyper pen's open contour, if drawing.
    pub(crate) hyper_contour: Option<usize>,
    /// Alt-hover segment preview (select tool), in glyph space.
    pub(crate) segment_hover: Option<kurbo::PathSeg>,
    /// The last flip/rotate, re-applied by duplicate-repeat.
    pub(crate) last_transform: Option<Affine>,
    /// The selected component of the open glyph, if any.
    pub(crate) selected_component: Option<usize>,
    /// Sidebearing edge under the cursor (false = left, true = right).
    pub(crate) sidebearing_hover: Option<bool>,
    /// Guide under the cursor: (local, index).
    pub(crate) guide_hover: Option<(bool, usize)>,
    /// Locked nodes, session-scoped: unselectable and undraggable
    /// until unlocked. This is node locking in Glyphs.
    pub(crate) locked_points: HashSet<(usize, usize)>,
    /// Mouse position in window coords, for pen previews.
    pub(crate) pointer: Option<Point<gpui::Pixels>>,
    /// The shared `runebender_core` viewport (design Y-up, screen
    /// Y-down).
    pub(crate) viewport: ViewPort,
    /// The viewport has been fitted to the canvas; false refits on
    /// the next paint.
    pub(crate) initialized: bool,
    /// The active tool.
    pub(crate) tool: Tool,
    /// Pen-tool drawing state, while a contour is open.
    pub(crate) pen: Option<PenState>,
    /// Shapes tool draws ellipses instead of rectangles.
    pub(crate) shape_ellipse: bool,
    /// The selected points, as `(contour, point)` indices.
    pub(crate) selected: HashSet<(usize, usize)>,
    /// Selected anchors, in the order they were picked. A selection
    /// may hold points and anchors at once; the last anchor picked
    /// is the primary the panels read.
    pub(crate) selected_anchors: Vec<usize>,
    /// Last cursor position in design space (for A = add anchor).
    pub(crate) cursor: (f64, f64),
    /// The mouse gesture in progress, if any.
    pub(crate) drag: Option<Drag>,
    /// Canvas bounds in window coordinates, written during paint so
    /// mouse handlers can map window→design coordinates.
    pub(crate) bounds: Arc<Mutex<Bounds<gpui::Pixels>>>,
}

impl EditorState {
    /// The anchor the side panels edit: the last one picked.
    pub(crate) fn selected_anchor(&self) -> Option<usize> {
        self.selected_anchors.last().copied()
    }

    /// A fresh state: select tool, empty selection, default
    /// viewport.
    pub(crate) fn new() -> Self {
        Self {
            sort_offset: (0.0, 0.0),
            previous_tool: Tool::Select,
            hyper_contour: None,
            segment_hover: None,
            last_transform: None,
            selected_component: None,
            sidebearing_hover: None,
            guide_hover: None,
            locked_points: HashSet::new(),
            pointer: None,
            viewport: ViewPort::new(),
            initialized: false,
            tool: Tool::Select,
            pen: None,
            shape_ellipse: false,
            selected: HashSet::new(),
            selected_anchors: Vec::new(),
            cursor: (0.0, 0.0),
            drag: None,
            bounds: Arc::new(Mutex::new(Bounds::default())),
        }
    }

    /// The design-to-local-pixels transform, in the active sort's
    /// glyph space. When the text tool has other sorts in the
    /// buffer, the open glyph sits at its layout position; the
    /// offset keeps every tool (points, pen, shapes, marquee)
    /// working in glyph-local coordinates.
    pub(crate) fn transform(&self) -> Affine {
        self.viewport.affine() * Affine::translate(self.sort_offset)
    }

    /// The viewport's zoom factor (design units to pixels).
    pub(crate) fn zoom(&self) -> f64 {
        self.viewport.zoom
    }

    /// Converts a window position to local canvas pixels.
    pub(crate) fn window_to_local(&self, pos: Point<gpui::Pixels>) -> kurbo::Point {
        let origin = self.bounds.lock().expect("the canvas bounds lock").origin;
        let lx: f32 = (pos.x - origin.x).into();
        let ly: f32 = (pos.y - origin.y).into();
        kurbo::Point::new(lx as f64, ly as f64)
    }

    /// Converts a window position to design coordinates.
    pub(crate) fn window_to_design(&self, pos: Point<gpui::Pixels>) -> (f64, f64) {
        let p = self.viewport.screen_to_design(self.window_to_local(pos));
        (p.x - self.sort_offset.0, p.y - self.sort_offset.1)
    }

    /// Fits the viewport to the canvas around the glyph's metrics
    /// and marks the state initialized.
    pub(crate) fn fit(&mut self, advance: f64, ascender: f64, descender: f64) {
        let bounds = *self.bounds.lock().expect("the canvas bounds lock");
        let w: f32 = bounds.size.width.into();
        let h: f32 = bounds.size.height.into();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.viewport
            .fit_to_canvas(w as f64, h as f64, advance, ascender, descender, 0.6);
        self.initialized = true;
    }
}

/// What the window shows: the font overview or one glyph's
/// editor.
pub(crate) enum Mode {
    /// The font overview.
    Grid,
    /// The edit view, open on the glyph at this index.
    Editor(usize),
}

/// The category rows, in web order. Labels double as the keys for
/// core's `category_subfilters`.
pub(crate) const SIDEBAR_CATEGORIES: [(runebender_core::analysis::category::GlyphCategory, &str);
    8] = {
    use runebender_core::analysis::category::GlyphCategory as GC;
    [
        (GC::All, "All"),
        (GC::Letter, "Letter"),
        (GC::Number, "Number"),
        (GC::Punctuation, "Punctuation"),
        (GC::Symbol, "Symbol"),
        (GC::Mark, "Mark"),
        (GC::Separator, "Separator"),
        (GC::Other, "Other"),
    ]
};

/// What the sidebar has selected. This is `GlyphSidebarFilter` in
/// the web editor.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum SidebarFilter {
    /// No filter: every glyph.
    All,
    /// One category row.
    Category(runebender_core::analysis::category::GlyphCategory),
    /// A subfilter under a category row.
    Subfilter(
        runebender_core::analysis::category::GlyphCategory,
        &'static str,
    ),
    /// A script group row (index into core's language groups).
    LanguageGroup(usize),
    /// One language under a script group: (group, language) indices.
    Language(usize, usize),
    /// A built-in smart filter (index into core's builtin filters).
    Builtin(usize),
    /// A user-saved search (index into the font's saved-filter list).
    Saved(usize),
}

/// Glyph counts for every sidebar row, computed once per font state.
pub(crate) struct SidebarCounts {
    /// One count per category row, in `SIDEBAR_CATEGORIES` order.
    pub(crate) categories: Vec<usize>,
    /// Counts keyed by (category row, subfilter) indices.
    pub(crate) subfilters: HashMap<(usize, usize), usize>,
    /// One count per script group.
    pub(crate) groups: Vec<usize>,
    /// Counts per group, then per language within it.
    pub(crate) languages: Vec<Vec<usize>>,
    /// Missing-target counts per (group, filter); 0 = complete or
    /// not target-bearing.
    pub(crate) missing: Vec<Vec<usize>>,
    /// One count per built-in smart filter.
    pub(crate) builtins: Vec<usize>,
    /// One count per user-saved search.
    pub(crate) saved: Vec<usize>,
}

/// One edit tab: the open glyph, plus the parked editor state and
/// text buffer.
///
/// The glyph is stored by name, so the tab survives renames and
/// master switches. The active tab's live state lives in
/// `Workspace::editor` and `edit_buffer`; its slot here is stale
/// until the next switch parks it back.
pub(crate) struct EditSession {
    /// The open glyph, by name.
    pub(crate) glyph_name: String,
    /// The parked editor state.
    pub(crate) editor: EditorState,
    /// The parked text buffer.
    pub(crate) buffer: runebender_core::text::buffer::TextBuffer,
}

/// Every glyph carrying one anchor name: marks (name, x, y), bases,
/// and mark carriers.
pub(crate) type AnchorFamily = (
    Vec<(String, f64, f64)>,
    Vec<(String, f64, f64)>,
    Vec<(String, f64, f64)>,
);

/// A blurred preview frame with the key it was rendered for.
pub(crate) type BlurFrame = (u64, Arc<gpui::RenderImage>);

/// The whole editor: the open project, the edit sessions, and every
/// view's state. `wiring.rs` builds it; the window renders it.
pub(crate) struct Workspace {
    /// The open font project; None when nothing loaded.
    pub(crate) project: Option<Project>,
    /// Why the project failed to open, shown in the status bar.
    pub(crate) load_error: Option<SharedString>,
    /// The grid's primary selected glyph, by index.
    pub(crate) selected: Option<usize>,
    /// The glyph whose edit session the tab strip returns to after
    /// the Font tab switched back to the overview.
    pub(crate) last_editor: Option<usize>,
    /// Edit tabs, Glyphs-style. Empty until a glyph is first opened.
    pub(crate) sessions: Vec<EditSession>,
    /// Index of the active edit tab in `sessions`.
    pub(crate) active_session: usize,
    /// A run of arrow-key nudges is in progress: they share one undo
    /// step until something else happens.
    pub(crate) nudging: bool,
    /// Decoded glyph background images from the UFO images store,
    /// keyed by file name; None caches a failed decode. Behind a
    /// mutex because rendering (which fills it) holds &self.
    pub(crate) glyph_image_cache: Arc<Mutex<HashMap<String, Option<Arc<gpui::RenderImage>>>>>,
    /// What the window shows: overview or editor.
    pub(crate) mode: Mode,
    /// The active session's live editor state.
    pub(crate) editor: EditorState,
    /// The editor's text buffer, owned by the text tool. The open
    /// glyph is the active sort; other sorts render as filled
    /// context around it, the model the web and xilem editors share.
    pub(crate) edit_buffer: runebender_core::text::buffer::TextBuffer,
    /// Folded sidebar sections, keyed by title.
    pub(crate) collapsed_sections: HashSet<&'static str>,
    /// Masters drawn as dim reference underlays in the editor
    /// (the layer rows' eye toggles).
    pub(crate) reference_layers: HashSet<usize>,
    /// Edit > Show All Masters: every master overlaid in the edit
    /// view. Clicking any master's node switches to that master
    /// with the node selected.
    pub(crate) show_all_masters: bool,
    /// The left sidebar is hidden. The header button toggles it,
    /// as in Glyphs.
    pub(crate) left_collapsed: bool,
    /// In-window menu bar for platforms without a native one
    /// (Windows, Linux, the browser).
    #[cfg(not(target_os = "macos"))]
    pub(crate) app_menu_bar: gpui::Entity<widgets::menu_bar::MenuBar>,
    /// The window's focus handle; keystrokes route through it.
    pub(crate) focus_handle: gpui::FocusHandle,
    /// A transient message for the status bar.
    pub(crate) status_note: Option<SharedString>,
    /// Wall-clock time of the last save, for the header.
    pub(crate) last_save_label: Option<SharedString>,
    /// Palette index the next color layer is assigned.
    pub(crate) color_selected: usize,
    /// Paint the color layers stacked in the editor.
    pub(crate) show_color_preview: bool,
    /// Draw node trajectories + velocity dots across the first axis
    /// (higher-order interpolation view).
    pub(crate) show_trajectories: bool,
    /// The intermediate point being dragged right now (id, Q),
    /// painted live and committed + baked on mouse-up.
    pub(crate) hoi_live: Option<((usize, usize), (f64, f64))>,
    /// The shaping inspector's focused cluster (carrier sort index).
    pub(crate) shaping_focus: Option<usize>,
    /// Ghost every attachable mark on the open glyph's anchors.
    /// This is the mark cloud in Glyphs.
    pub(crate) show_mark_cloud: bool,
    /// Preview feature overrides: tag → forced on/off. Absent tags
    /// keep the shaper's defaults.
    pub(crate) feature_overrides: HashMap<String, bool>,
    /// Preview shaping locale: (script tag, BCP 47 language), e.g.
    /// ("arab", "ur"). None = direction-derived defaults.
    pub(crate) shaping_locale: Option<(String, String)>,
    /// Seed for the Roughen command, bumped on each apply so
    /// results differ.
    pub(crate) roughen_seed: u64,
    /// Unapplied edits in the features editor: the refresh keeps its
    /// hands off until Apply or Revert.
    pub(crate) features_edited: bool,
    /// The last Apply's compile verdict, shown under the editor.
    pub(crate) features_status: Option<SharedString>,
    /// The open right-click menu over the canvas, if any.
    pub(crate) context_menu: Option<ContextMenu>,
    /// The Selection panel's 9-point reference for numeric move
    /// and scale. This is the coordinate quadrant in the web
    /// editor.
    pub(crate) coord_quadrant: runebender_core::outline::path::Quadrant,
    /// Curve overlays. This is `CurvePanel` in the web editor.
    pub(crate) curve_comb: bool,
    /// Mark curvature continuity where curve segments join.
    pub(crate) curve_continuity: bool,
    /// Measure-tool HUD layers. These are `SelectPanel`'s
    /// `MeasureOptions` in the web editor.
    pub(crate) measure_opts: MeasureOpts,
    /// Show the UFO background layer as a quiet outline.
    pub(crate) show_background: bool,
    /// Per-glyph UFO layers drawn as underlays (layer names with the
    /// eye on), beyond the default and background layers.
    pub(crate) visible_glyph_layers: HashSet<String>,
    /// Another glyph ghosted behind the drawing for comparison.
    pub(crate) reference_glyph: Option<String>,
    /// Sliders for non-degenerate designspace axes: (axis index,
    /// slider), created lazily in render.
    pub(crate) axis_sliders: Vec<(usize, gpui::Entity<widgets::slider::SliderState>)>,
    /// Internal outline clipboard: whole contours.
    pub(crate) clipboard: Vec<norad::Contour>,
    /// Web host connection (server base + file ETags), when the page
    /// was opened with ?server=.
    #[cfg(target_family = "wasm")]
    pub(crate) web_host: Option<web_host::WebHost>,
    /// Filesystem watcher over the open masters' UFO directories.
    pub(crate) _watcher: Option<notify::RecommendedWatcher>,
    /// Set at save time so the watcher ignores our own writes.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) last_save: Arc<Mutex<web_time::Instant>>,
    /// A selected kern pair in the preview strip: indices into the
    /// resolved preview line (glyph indices of the pair).
    pub(crate) _subscriptions: Vec<gpui::Subscription>,
    /// The glyph grid's view state.
    pub(crate) grid: GridState,
    /// The left sidebar's state.
    pub(crate) sidebar: SidebarState,
    /// The preview strip's state.
    pub(crate) preview: PreviewState,
    /// The Local AI panel's state.
    pub(crate) models: ModelsState,
    /// The inspector's input fields.
    pub(crate) inputs: InputFields,
}

/// The glyph grid's view state: cell size, scroll, order, selection.
pub(crate) struct GridState {
    /// Grid sort: false = by name, true = by unicode. Unicode is
    /// the web editor's default.
    pub(crate) sort_unicode: bool,
    /// Grid cell size in px, driven by the bottom bar's zoom slider.
    /// This is the *target*: cells stretch from it to fill the row.
    pub(crate) cell_size: f32,
    /// Measured size of the glyph grid's scroll viewport. Columns and
    /// row height are solved against it so rows fill the width and
    /// divide the height evenly (no half row at the bottom edge).
    pub(crate) viewport: gpui::Size<gpui::Pixels>,
    /// The glyphs the filters and the search leave, in display order.
    /// Rebuilt when the inputs change rather than on every frame: it
    /// filters and sorts the whole font, which is far too much work to
    /// repeat for a mouse move.
    pub(crate) order: Option<Arc<Vec<usize>>>,
    /// What `glyph_order` was built from.
    pub(crate) order_key: Option<OrderKey>,
    /// First visible row of each grid. Scrolling moves whole rows.
    pub(crate) scroll_row: usize,
    /// The bottom bar's cell-zoom slider, built lazily in render.
    pub(crate) cell_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    /// Multi-selected glyph names (grid cmd/shift-click); `selected`
    /// stays the primary.
    pub(crate) multi_selected: HashSet<String>,
    /// The font view mode: grid, detail, list, or matrix.
    pub(crate) view_mode: FontViewMode,
}

/// The left sidebar: filter, search, expansion, scroll, and its inputs.
pub(crate) struct SidebarState {
    /// What the sidebar has selected.
    pub(crate) filter: SidebarFilter,
    /// Names matched by the current sidebar filter (None = all).
    pub(crate) matches: Option<HashSet<String>>,
    /// Per-row glyph counts, rebuilt on load/reload/master switch.
    pub(crate) counts: Option<SidebarCounts>,
    /// Script group rows unfolded to show their languages.
    pub(crate) expanded_scripts: HashSet<usize>,
    /// Category rows unfolded to show their subfilters.
    pub(crate) expanded_categories: HashSet<usize>,
    /// The same, for the editor sidebar's mini glyph grid.
    pub(crate) viewport: gpui::Size<gpui::Pixels>,
    /// The search pattern, compiled once instead of per glyph.
    pub(crate) search_re: Option<regex::Regex>,
    /// First visible row of the mini grid.
    pub(crate) scroll_row: usize,
    /// Which editor-sidebar tab is up: 0 glyphs, 1 shapes, 2 axes,
    /// 3 chat.
    pub(crate) tab: u8,
    /// Target cell size for the editor sidebar's mini grid.
    pub(crate) cell_size: f32,
    /// The mini grid's cell-size slider, built lazily in render.
    pub(crate) slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    /// The search box's widget state.
    pub(crate) search_input: gpui::Entity<widgets::input::InputState>,
    /// The search text, lowercased on each edit.
    pub(crate) search_query: String,
    /// Search scope: 0 = all, 1 = name, 2 = unicode.
    pub(crate) search_mode: u8,
    /// Treat the search as a regular expression.
    pub(crate) search_regex: bool,
    /// Match case instead of ignoring it.
    pub(crate) search_case: bool,
    /// Parsed predicate query, rebuilt when the search changes.
    pub(crate) search_predicates: Option<Vec<SearchPred>>,
}

/// The preview strip's state.
pub(crate) struct PreviewState {
    /// The text preview strip under the editor is showing.
    pub(crate) visible: bool,
    /// How far the strip is blurred, in pixels; 0 draws it sharp.
    pub(crate) blur: f32,
    /// The last blurred frame, kept so dragging a point does not
    /// re-rasterize the preview on every mouse move.
    pub(crate) blur_cache: Arc<Mutex<Option<BlurFrame>>>,
    /// Draw light-on-dark instead of dark-on-light.
    pub(crate) invert: bool,
    /// The blur slider, built lazily in render.
    pub(crate) blur_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    /// Which built-in sample string the buffer shows.
    pub(crate) sample_index: usize,
}

/// The Local AI panel's state: the model on disk and its controls.
pub(crate) struct ModelsState {
    /// Scales what a model predicts. A model can be right about which
    /// way a point moves and short on how far, which looks like a
    /// prediction that is too light.
    pub(crate) strength: f64,
    /// The chosen model directory, kept so a run does not re-ask.
    pub(crate) dir: Option<PathBuf>,
    /// What the directory says it is, for the panel.
    pub(crate) summary: Option<SharedString>,
    /// Last judgement: glyph, model error, baseline error.
    pub(crate) score: Option<(SharedString, f64, f64)>,
    /// What font-ml is doing right now, while it runs.
    pub(crate) busy: Option<SharedString>,
    /// The proposal waiting in the active master, if any.
    pub(crate) proposal: Option<runebender_core::document::proposal::ProposalSummary>,
    /// The strength slider, built lazily in render.
    pub(crate) strength_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
}

/// Every input field the inspector owns. The widgets are built in
/// `wiring.rs`; what typing in one does lives in `edit/inspector.rs`.
pub(crate) struct InputFields {
    /// The editor's metric and Selection fields.
    pub(crate) metric: MetricInputs,
    /// The Font Info section's fields.
    pub(crate) font_info: FontInfoInputs,
    /// The Kerning section's inputs.
    pub(crate) kern: KernInputs,
    /// Slant angle field in the Transformations section (degrees).
    pub(crate) slant: gpui::Entity<widgets::input::InputState>,
    /// Stroke width field in the Transformations section (units).
    pub(crate) stroke: gpui::Entity<widgets::input::InputState>,
    /// Offset field: bolder (positive) or lighter (negative) units.
    pub(crate) offset: gpui::Entity<widgets::input::InputState>,
    /// Fit Curve percentage field in the Curves section.
    pub(crate) fit: gpui::Entity<widgets::input::InputState>,
    /// Hex field that appends a color to the CPAL palette.
    pub(crate) color_hex: gpui::Entity<widgets::input::InputState>,
    /// Ease amount field: Enter bakes interpolation timing into a
    /// brace layer at the preview location.
    pub(crate) ease: gpui::Entity<widgets::input::InputState>,
    /// Extrude field ("offset,angle"; k-prefix keeps the front).
    pub(crate) extrude: gpui::Entity<widgets::input::InputState>,
    /// Roughen field ("segment,h,v"); reseeded per apply.
    pub(crate) roughen: gpui::Entity<widgets::input::InputState>,
    /// The Instances editor field under the axis sliders: Enter
    /// renames the instance at the preview location, or adds one.
    pub(crate) instance_name: gpui::Entity<widgets::input::InputState>,
    /// The Features section's features.fea editor (grid mode).
    pub(crate) features: gpui::Entity<widgets::input::InputState>,
    /// The Glyph panel's fields.
    pub(crate) glyph: GlyphInputs,
    /// Names the glyph ghosted behind the drawing for comparison.
    pub(crate) reference_glyph: gpui::Entity<widgets::input::InputState>,
    /// Component-glyph name typed in the context menu (Add
    /// Component).
    pub(crate) component_name: gpui::Entity<widgets::input::InputState>,
    /// Corner-glyph name typed in the context menu (Apply Corner…).
    pub(crate) corner_name: gpui::Entity<widgets::input::InputState>,
    /// Note text typed in the context menu (Annotate: Note…).
    pub(crate) annotation: gpui::Entity<widgets::input::InputState>,
    /// Smart-axis definition on the open part glyph ("Width,0,100").
    pub(crate) smart_axis: gpui::Entity<widgets::input::InputState>,
    /// New kerning group from the grid selection: "o" (kern1) or
    /// "|o" (kern2).
    pub(crate) group_name: gpui::Entity<widgets::input::InputState>,
    /// New avar pair on the first axis: "user,design".
    pub(crate) axis_map: gpui::Entity<widgets::input::InputState>,
    /// The selected smart component's value on its first axis.
    pub(crate) smart_value: gpui::Entity<widgets::input::InputState>,
    /// Renames the selected anchor on Enter.
    pub(crate) anchor_name: gpui::Entity<widgets::input::InputState>,
}

/// Which measurement-HUD layers the Measure tool draws. Every
/// layer off returns the plain editor; the panel is purely
/// additive. This is `MeasureOptions` in the web editor.
#[derive(Clone, Copy)]
pub(crate) struct MeasureOpts {
    /// Tint outline segments, curves, and handles by popcount.
    pub(crate) colorize: bool,
    /// Label Bézier handle lengths.
    pub(crate) handles: bool,
    /// Label straight outline segment lengths.
    pub(crate) segments: bool,
    /// Draw + label stem/counter/height spans (dimension lines).
    pub(crate) spans: bool,
    /// Draw + label left/right side bearings.
    pub(crate) sidebearings: bool,
    /// Label every curve segment with the size of its own bounding
    /// box, so a glyph's curves can be compared at a glance.
    pub(crate) sizes: bool,
    /// Spell lengths as sums of powers of two (`96 = 64+32`).
    pub(crate) popcount: bool,
}

impl Default for MeasureOpts {
    fn default() -> Self {
        Self {
            colorize: false,
            handles: false,
            segments: false,
            spans: false,
            sidebearings: false,
            sizes: false,
            popcount: true,
        }
    }
}

/// What the Measure menu shows as ticked. The menu is built outside
/// the view, so the live options are mirrored here whenever they
/// change.
pub(crate) static MEASURE_MENU: Mutex<MeasureOpts> = Mutex::new(MeasureOpts {
    colorize: false,
    handles: false,
    segments: false,
    spans: false,
    sidebearings: false,
    sizes: false,
    popcount: true,
});

impl MeasureOpts {
    /// Whether any measurement layer is on; popcount alone does not
    /// count.
    pub(crate) fn any(&self) -> bool {
        self.colorize
            || self.handles
            || self.segments
            || self.spans
            || self.sidebearings
            || self.sizes
    }

    /// Formats a length: popcount spelling when enabled, plain
    /// digits otherwise.
    pub(crate) fn label(&self, value: i64) -> String {
        if self.popcount {
            runebender_core::analysis::measure::label(value)
        } else {
            value.to_string()
        }
    }
}

/// Editable glyph-data fields in the Glyph panel.
pub(crate) struct GlyphInputs {
    /// The glyph's name; Enter renames it.
    pub(crate) name: gpui::Entity<widgets::input::InputState>,
    /// The glyph's codepoint, in hex.
    pub(crate) unicode: gpui::Entity<widgets::input::InputState>,
    /// The left kerning group (kern1).
    pub(crate) group_l: gpui::Entity<widgets::input::InputState>,
    /// The right kerning group (kern2).
    pub(crate) group_r: gpui::Entity<widgets::input::InputState>,
    /// Free-text glyph note, the UFO glif `note` element. This is
    /// the note field in Glyphs, which shows it as a tooltip in its
    /// font view.
    pub(crate) note: gpui::Entity<widgets::input::InputState>,
    /// Shape-switch point: Enter creates the `.bold` alternate and
    /// the designspace rule at this axis value. This is a bracket
    /// layer in Glyphs.
    pub(crate) switch_at: gpui::Entity<widgets::input::InputState>,
    /// Metrics keys ("=n", "=|o", "=n+10"): linked sidebearings,
    /// synced across every master.
    pub(crate) lsb_key: gpui::Entity<widgets::input::InputState>,
    /// The same, for the right sidebearing.
    pub(crate) rsb_key: gpui::Entity<widgets::input::InputState>,
    /// Export (production) name, written to public.postscriptNames
    /// in every master's lib; ufo2ft renames on compile.
    pub(crate) production: gpui::Entity<widgets::input::InputState>,
}

/// The editor's Width / LSB / RSB fields and the Selection
/// section's coordinate fields.
pub(crate) struct MetricInputs {
    /// The advance width field.
    pub(crate) width: gpui::Entity<widgets::input::InputState>,
    /// The left sidebearing field.
    pub(crate) lsb: gpui::Entity<widgets::input::InputState>,
    /// The right sidebearing field.
    pub(crate) rsb: gpui::Entity<widgets::input::InputState>,
    /// Selection reference coordinates and size (Selection section).
    pub(crate) x: gpui::Entity<widgets::input::InputState>,
    /// The reference point's y coordinate.
    pub(crate) y: gpui::Entity<widgets::input::InputState>,
    /// The selection's width.
    pub(crate) w: gpui::Entity<widgets::input::InputState>,
    /// The selection's height.
    pub(crate) h: gpui::Entity<widgets::input::InputState>,
}

/// Editable fields in the Font Info section (grid mode). Each commits
/// on Enter and writes fontinfo.plist through the normal save path.
pub(crate) struct FontInfoInputs {
    /// The family name.
    pub(crate) family: gpui::Entity<widgets::input::InputState>,
    /// The style name.
    pub(crate) style: gpui::Entity<widgets::input::InputState>,
    /// Units per em.
    pub(crate) upm: gpui::Entity<widgets::input::InputState>,
    /// The italic angle, in degrees.
    pub(crate) italic_angle: gpui::Entity<widgets::input::InputState>,
    /// The ascender, in design units.
    pub(crate) ascender: gpui::Entity<widgets::input::InputState>,
    /// The descender, in design units (negative below the baseline).
    pub(crate) descender: gpui::Entity<widgets::input::InputState>,
    /// The x-height, in design units.
    pub(crate) x_height: gpui::Entity<widgets::input::InputState>,
    /// The cap height, in design units.
    pub(crate) cap_height: gpui::Entity<widgets::input::InputState>,
    /// PostScript hinting data per master: alignment zones (blue
    /// values in pairs) and standard stems, comma-separated lists.
    pub(crate) blue_values: gpui::Entity<widgets::input::InputState>,
    /// Descender-side alignment zones, in pairs.
    pub(crate) other_blues: gpui::Entity<widgets::input::InputState>,
    /// Standard horizontal stem widths.
    pub(crate) stems_h: gpui::Entity<widgets::input::InputState>,
    /// Standard vertical stem widths.
    pub(crate) stems_v: gpui::Entity<widgets::input::InputState>,
    /// The OS/2 and hhea vertical metrics (typo/hhea/win), the
    /// parameters the Google Fonts vertical-metrics checks read.
    pub(crate) typo_asc: gpui::Entity<widgets::input::InputState>,
    /// OS/2 `sTypoDescender`.
    pub(crate) typo_desc: gpui::Entity<widgets::input::InputState>,
    /// OS/2 `sTypoLineGap`.
    pub(crate) typo_gap: gpui::Entity<widgets::input::InputState>,
    /// The `hhea` ascender.
    pub(crate) hhea_asc: gpui::Entity<widgets::input::InputState>,
    /// The `hhea` descender.
    pub(crate) hhea_desc: gpui::Entity<widgets::input::InputState>,
    /// The `hhea` line gap.
    pub(crate) hhea_gap: gpui::Entity<widgets::input::InputState>,
    /// OS/2 `usWinAscent`.
    pub(crate) win_asc: gpui::Entity<widgets::input::InputState>,
    /// OS/2 `usWinDescent`.
    pub(crate) win_desc: gpui::Entity<widgets::input::InputState>,
}

/// The Kerning section's inputs: a live filter over the pair list,
/// and a first/second/value editor that commits on Enter.
pub(crate) struct KernInputs {
    /// Live filter over the pair list.
    pub(crate) filter: gpui::Entity<widgets::input::InputState>,
    /// The pair's first glyph or group.
    pub(crate) first: gpui::Entity<widgets::input::InputState>,
    /// The pair's second glyph or group.
    pub(crate) second: gpui::Entity<widgets::input::InputState>,
    /// The pair's kerning value.
    pub(crate) value: gpui::Entity<widgets::input::InputState>,
}

/// Which Font Info field an input commits to.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum FontInfoField {
    /// The family name.
    Family,
    /// The style name.
    Style,
    /// Units per em.
    Upm,
    /// The italic angle.
    ItalicAngle,
    /// The ascender.
    Ascender,
    /// The descender.
    Descender,
    /// The x-height.
    XHeight,
    /// The cap height.
    CapHeight,
    /// OS/2 `sTypoAscender`.
    TypoAscender,
    /// OS/2 `sTypoDescender`.
    TypoDescender,
    /// OS/2 `sTypoLineGap`.
    TypoLineGap,
    /// The `hhea` ascender.
    HheaAscender,
    /// The `hhea` descender.
    HheaDescender,
    /// The `hhea` line gap.
    HheaLineGap,
    /// OS/2 `usWinAscent`.
    WinAscent,
    /// OS/2 `usWinDescent`.
    WinDescent,
    /// PostScript blue values (alignment zones).
    BlueValues,
    /// PostScript other blues.
    OtherBlues,
    /// Standard horizontal stems.
    StemsH,
    /// Standard vertical stems.
    StemsV,
}

/// How a grid of glyph cells fits its pane: cell size, and how many
/// columns and whole rows are on screen.
#[derive(Clone, Copy)]
pub(crate) struct GridFit {
    /// Cell width in pixels, stretched from the target to fill the
    /// row.
    pub(crate) cell_w: f32,
    /// Cell height in pixels.
    pub(crate) cell_h: f32,
    /// Columns per row.
    pub(crate) cols: usize,
    /// Whole rows on screen.
    pub(crate) rows: usize,
}

impl GridFit {
    /// Exact width of a full row of cells, gaps included.
    pub(crate) fn content_w(&self) -> f32 {
        self.cell_w * self.cols as f32 + GRID_GAP * (self.cols - 1) as f32
    }
}

/// Default target cell size for the glyph grid, in pixels.
pub(crate) const CELL: f32 = 96.0;

/// Target cell size for the editor sidebar's mini grid.
pub(crate) const MINI_CELL: f32 = 44.0;

/// Height of every bottom bar, so the ones in neighbouring columns
/// line up across the divider.
pub(crate) const BOTTOM_BAR_H: f32 = 28.0;

/// Square buttons in a bottom bar, sized so the space above, below and
/// beside them is the same.
pub(crate) const BAR_BUTTON: f32 = 20.0;

/// Wheel zoom response and limits, matching the web editor.
pub(crate) const ZOOM_PER_PIXEL: f64 = 0.0015;

/// Smallest allowed zoom factor.
pub(crate) const ZOOM_MIN: f64 = 1e-3;

/// Largest allowed zoom factor.
pub(crate) const ZOOM_MAX: f64 = 1e4;

/// One press of the zoom keys.
pub(crate) const ZOOM_KEY_STEP: f64 = 1.1;

/// Height of a header tab, and the side of the square icon buttons
/// that sit beside tabs in the header and the status bar.
pub(crate) const TAB_H: f32 = 24.0;

/// Gap between grid cells, and the grid's inner padding.
pub(crate) const GRID_GAP: f32 = 8.0;

/// The grid's horizontal inner padding, in pixels.
pub(crate) const GRID_PAD: f32 = 12.0;

/// The grid's vertical inner padding, in pixels.
pub(crate) const GRID_PAD_Y: f32 = 8.0;

/// The sidebar's mini grid is narrow: it spares less padding, but the
/// fit is solved the same way.
pub(crate) const GRID_PAD_SM: f32 = 6.0;

/// Hit-test radius in screen pixels for segments, guides,
/// metric edges, and components.
pub(crate) const HIT_RADIUS_PX: f64 = 10.0;

/// Hit-test radius in screen pixels for points. Points are easier
/// to grab than segments, so they get a wider radius than
/// `HIT_RADIUS_PX`. This is `SELECT_POINT_HIT_DISTANCE` in the web
/// editor.
pub(crate) const POINT_HIT_RADIUS_PX: f64 = 16.0;

/// The config file's contents, read once before the window opens.
///
/// A `OnceLock` rather than a re-read per call: the file is read at
/// startup and changing it means restarting, which is the same promise
/// the theme menu makes.
#[cfg(not(target_family = "wasm"))]
pub(crate) static CONFIG: std::sync::OnceLock<config::Config> = std::sync::OnceLock::new();
