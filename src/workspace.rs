// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The editor's state: the `Workspace` struct and the types it is made of.
//!
//! Everything the window shows comes from here. The methods live in
//! `view/`, `edit/`, and `platform/`, one file per concern, and
//! `startup.rs` builds the first value.

use crate::*;

/// Font View's three modes (Glyphs 4): grid, detail, list.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum FontViewMode {
    Grid,
    Detail,
    List,
    /// The positional-forms matrix: Arabic review, isol/init/
    /// medi/fina as columns per base letter.
    Matrix,
}

/// Built-in sample strings (View > Next Sample String): spacing
/// control strings and kern words, cycled around the open glyph.
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
    Width,
    Lsb,
    Rsb,
}

/// The active editor tool.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    Select,
    Pen,
    Shapes,
    Text,
    Knife,
    Preview,
    HyperPen,
    Measure,
}

/// Pen-tool drawing state: the open contour and the outgoing handle
/// of the previously placed point (set by click-dragging it).
pub(crate) struct PenState {
    pub(crate) contour: usize,
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
        start: (f64, f64),
        originals: std::collections::HashMap<(usize, usize), (f64, f64)>,
        /// Selected anchors travel with the points (web moves points
        /// and anchors on one delta): index and start position each.
        anchor: Vec<(usize, (f64, f64))>,
    },
    /// Manual kerning drag in the text buffer (engine session).
    TextKern,
    /// Alt-drag pans the viewport (select tool). Window-space anchor.
    Pan { last: (f64, f64) },
    /// Dragging a sidebearing edge (false = left, true = right).
    Sidebearing {
        right: bool,
        start_x: f64,
        applied: f64,
        start_width: f64,
    },
    /// Free transform from the selection bounding box: a handle
    /// scales about the opposite handle, the ring outside a corner
    /// rotates about the box centre. Shift constrains (proportional
    /// scale, 15-degree rotation steps).
    FreeTransform {
        /// The fixed point of the gesture, in design space.
        anchor: (f64, f64),
        /// Where the gesture began, in design space.
        start: (f64, f64),
        /// Rotation instead of scaling.
        rotate: bool,
        /// Which axes a scale handle drives (edge handles pin one).
        scale_x: bool,
        scale_y: bool,
        /// Every point's position when the gesture began.
        originals: std::collections::HashMap<(usize, usize), (f64, f64)>,
    },
    /// Dragging a node's HOI intermediate knob: the point id and
    /// the node's positions in the axis-end masters.
    HoiKnob {
        id: (usize, usize),
        a: (f64, f64),
        b: (f64, f64),
    },
    /// Dragging a guide: `local` picks the open glyph's guidelines
    /// over the master's fontinfo ones. Guides move live; the
    /// master is marked dirty as they move.
    Guide { local: bool, index: usize },
    /// Dragging the selected component.
    Component {
        index: usize,
        start: (f64, f64),
        orig: (f64, f64),
    },
    /// Knife line, in design space.
    Knife {
        start: (f64, f64),
        current: (f64, f64),
    },
    /// Rubber-band selection rectangle, in design space. `base` is
    /// what was selected when the drag began: the live selection is
    /// always that plus whatever the box now encloses.
    Marquee {
        start: (f64, f64),
        current: (f64, f64),
        base: std::collections::HashSet<(usize, usize)>,
        base_anchors: Vec<usize>,
    },
    /// Dragging out a rectangle/ellipse (shapes tool).
    Shape {
        start: (f64, f64),
        current: (f64, f64),
    },
    /// Measuring (measure tool).
    Measure {
        start: (f64, f64),
        current: (f64, f64),
    },
}

/// Editor viewport and interaction state, on the shared
/// `runebender_core` viewport (design Y-up ↔ screen Y-down).
/// A right-click context menu over the editor canvas (web
/// contourContextMenu).
pub(crate) struct ContextMenu {
    /// Position inside the canvas, in canvas-local pixels.
    pub(crate) at: Point<gpui::Pixels>,
    pub(crate) design: (f64, f64),
    pub(crate) contour: Option<usize>,
    pub(crate) contour_count: usize,
    pub(crate) start_point: Option<(usize, usize)>,
    pub(crate) anchor: Option<usize>,
    pub(crate) component: Option<(usize, bool)>,
    pub(crate) has_components: bool,
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
    /// Locked nodes (session-scoped): unselectable and undraggable
    /// until unlocked — Glyphs' node locking.
    pub(crate) locked_points: std::collections::HashSet<(usize, usize)>,
    /// Mouse position in window coords, for pen previews.
    pub(crate) pointer: Option<Point<gpui::Pixels>>,
    pub(crate) viewport: ViewPort,
    pub(crate) initialized: bool,
    pub(crate) tool: Tool,
    pub(crate) pen: Option<PenState>,
    /// Shapes tool draws ellipses instead of rectangles.
    pub(crate) shape_ellipse: bool,
    pub(crate) selected: std::collections::HashSet<(usize, usize)>,
    /// Selected anchors, in the order they were picked. A selection
    /// may hold points and anchors at once (web keeps both in one
    /// selection); the last one is the "primary" the panels read.
    pub(crate) selected_anchors: Vec<usize>,
    /// Last cursor position in design space (for A = add anchor).
    pub(crate) cursor: (f64, f64),
    pub(crate) drag: Option<Drag>,
    /// Undo/redo stacks of glyph snapshots for the open glyph.
    pub(crate) undo: Vec<GlyphSnapshot>,
    pub(crate) redo: Vec<GlyphSnapshot>,
    /// Canvas bounds in window coordinates, written during paint so
    /// mouse handlers can map window→design coordinates.
    pub(crate) bounds: Arc<Mutex<Bounds<gpui::Pixels>>>,
}

impl EditorState {
    /// The anchor the side panels edit: the last one picked.
    pub(crate) fn selected_anchor(&self) -> Option<usize> {
        self.selected_anchors.last().copied()
    }

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
            locked_points: std::collections::HashSet::new(),
            pointer: None,
            viewport: ViewPort::new(),
            initialized: false,
            tool: Tool::Select,
            pen: None,
            shape_ellipse: false,
            selected: std::collections::HashSet::new(),
            selected_anchors: Vec::new(),
            cursor: (0.0, 0.0),
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            bounds: Arc::new(Mutex::new(Bounds::default())),
        }
    }

    /// design → local pixels, in the active sort's glyph space.
    /// When the text tool has other sorts in the buffer, the open
    /// glyph sits at its layout position; the offset keeps every
    /// tool (points, pen, shapes, marquee) working in glyph-local
    /// coordinates.
    pub(crate) fn transform(&self) -> Affine {
        self.viewport.affine() * Affine::translate(self.sort_offset)
    }

    pub(crate) fn zoom(&self) -> f64 {
        self.viewport.zoom
    }

    /// window position → local canvas pixels
    pub(crate) fn window_to_local(&self, pos: Point<gpui::Pixels>) -> kurbo::Point {
        let origin = self.bounds.lock().unwrap().origin;
        let lx: f32 = (pos.x - origin.x).into();
        let ly: f32 = (pos.y - origin.y).into();
        kurbo::Point::new(lx as f64, ly as f64)
    }

    /// window position → design coordinates
    pub(crate) fn window_to_design(&self, pos: Point<gpui::Pixels>) -> (f64, f64) {
        let p = self.viewport.screen_to_design(self.window_to_local(pos));
        (p.x - self.sort_offset.0, p.y - self.sort_offset.1)
    }

    pub(crate) fn fit(&mut self, advance: f64, ascender: f64, descender: f64) {
        let bounds = *self.bounds.lock().unwrap();
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

pub(crate) enum Mode {
    Grid,
    Editor(usize),
}

/// The category rows, in web order. Labels double as the keys for
/// core's category_subfilters.
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

/// What the sidebar has selected (web GlyphSidebarFilter).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum SidebarFilter {
    All,
    Category(runebender_core::analysis::category::GlyphCategory),
    Subfilter(
        runebender_core::analysis::category::GlyphCategory,
        &'static str,
    ),
    LanguageGroup(usize),
    Language(usize, usize),
    Builtin(usize),
    /// A user-saved search (index into the font's saved-filter list).
    Saved(usize),
}

/// Glyph counts for every sidebar row, computed once per font state.
pub(crate) struct SidebarCounts {
    #[allow(dead_code)]
    pub(crate) total: usize,
    pub(crate) categories: Vec<usize>,
    pub(crate) subfilters: std::collections::HashMap<(usize, usize), usize>,
    pub(crate) groups: Vec<usize>,
    pub(crate) languages: Vec<Vec<usize>>,
    /// Missing-target counts per (group, filter); 0 = complete or
    /// not target-bearing.
    pub(crate) missing: Vec<Vec<usize>>,
    pub(crate) builtins: Vec<usize>,
    pub(crate) saved: Vec<usize>,
}

/// One edit tab: the open glyph (by name, so it survives renames
/// and master switches), plus the parked editor state and text
/// buffer. The ACTIVE tab's live state lives in `Workspace::editor`
/// and `edit_buffer`; its slot here is stale until the next switch
/// parks it back.
pub(crate) struct EditSession {
    pub(crate) glyph_name: String,
    pub(crate) editor: EditorState,
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

pub(crate) struct Workspace {
    pub(crate) project: Option<Project>,
    pub(crate) load_error: Option<SharedString>,
    pub(crate) selected: Option<usize>,
    /// The glyph whose edit session the tab strip returns to after
    /// the Font tab switched back to the overview.
    pub(crate) last_editor: Option<usize>,
    /// Edit tabs, Glyphs-style. Empty until a glyph is first opened.
    pub(crate) sessions: Vec<EditSession>,
    pub(crate) active_session: usize,
    pub(crate) sidebar_filter: SidebarFilter,
    /// Names matched by the current sidebar filter (None = all).
    pub(crate) sidebar_matches: Option<std::collections::HashSet<String>>,
    /// Per-row glyph counts, rebuilt on load/reload/master switch.
    pub(crate) sidebar_counts: Option<SidebarCounts>,
    pub(crate) expanded_scripts: std::collections::HashSet<usize>,
    pub(crate) expanded_categories: std::collections::HashSet<usize>,
    /// Grid sort: false = by name, true = by unicode (web default).
    pub(crate) sort_unicode: bool,
    /// A run of arrow-key nudges is in progress: they share one undo
    /// step until something else happens.
    pub(crate) nudging: bool,
    /// Text preview strip under the editor: whether it is showing, its
    /// type size in pixels, how far it is blurred (a spacing check),
    /// whether the colors are flipped, and how the line is aligned.
    pub(crate) preview_visible: bool,
    pub(crate) preview_blur: f32,
    /// The last blurred frame, kept so dragging a point does not
    /// re-rasterize the preview on every mouse move.
    pub(crate) preview_blur_cache: Arc<Mutex<Option<BlurFrame>>>,
    /// Decoded glyph background images from the UFO images store,
    /// keyed by file name; None caches a failed decode. Behind a
    /// mutex because rendering (which fills it) holds &self.
    pub(crate) glyph_image_cache:
        Arc<Mutex<std::collections::HashMap<String, Option<Arc<gpui::RenderImage>>>>>,
    pub(crate) preview_invert: bool,
    pub(crate) preview_blur_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    /// Grid cell size in px, driven by the bottom bar's zoom slider.
    /// This is the *target*: cells stretch from it to fill the row.
    pub(crate) grid_cell_size: f32,
    /// Measured size of the glyph grid's scroll viewport. Columns and
    /// row height are solved against it so rows fill the width and
    /// divide the height evenly (no half row at the bottom edge).
    pub(crate) grid_viewport: gpui::Size<gpui::Pixels>,
    /// The same, for the editor sidebar's mini glyph grid.
    pub(crate) sidebar_viewport: gpui::Size<gpui::Pixels>,
    /// The glyphs the filters and the search leave, in display order.
    /// Rebuilt when the inputs change rather than on every frame: it
    /// filters and sorts the whole font, which is far too much work to
    /// repeat for a mouse move.
    pub(crate) glyph_order: Option<Arc<Vec<usize>>>,
    /// What `glyph_order` was built from.
    pub(crate) order_key: Option<OrderKey>,
    /// The search pattern, compiled once instead of per glyph.
    pub(crate) search_re: Option<regex::Regex>,
    /// First visible row of each grid. Scrolling moves whole rows.
    pub(crate) grid_scroll_row: usize,
    pub(crate) sidebar_scroll_row: usize,
    /// Which editor-sidebar tab is up: 0 glyphs, 1 shapes, 2 axes,
    /// 3 chat.
    pub(crate) sidebar_tab: u8,
    /// Target cell size for the editor sidebar's mini grid.
    pub(crate) sidebar_cell_size: f32,
    pub(crate) sidebar_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    pub(crate) cell_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    pub(crate) mode: Mode,
    pub(crate) editor: EditorState,
    /// The editor's text buffer (the text tool): the open glyph is
    /// the active sort; other sorts render as filled context around
    /// it, exactly the web and xilem model.
    pub(crate) edit_buffer: runebender_core::text::buffer::TextBuffer,
    /// Keys route to the preview buffer (click the strip to focus,
    /// Escape to leave).
    /// Folded sidebar sections (by title).
    pub(crate) collapsed_sections: std::collections::HashSet<&'static str>,
    /// Masters drawn as dim reference underlays in the editor
    /// (the layer rows' eye toggles).
    pub(crate) reference_layers: std::collections::HashSet<usize>,
    /// Edit > Show All Masters: every master overlaid in the edit
    /// view, any master's node clickable (the click switches to that
    /// master with the node selected).
    pub(crate) show_all_masters: bool,
    /// Left sidebar hidden (header toggle, like the Glyphs one).
    pub(crate) left_collapsed: bool,
    /// In-window menu bar for platforms without a native one
    /// (Windows, Linux, the browser).
    #[cfg(not(target_os = "macos"))]
    pub(crate) app_menu_bar: gpui::Entity<widgets::menu_bar::MenuBar>,
    pub(crate) focus_handle: gpui::FocusHandle,
    /// Scales what a model predicts. A model can be right about which
    /// way a point moves and short on how far, which looks like a
    /// prediction that is too light.
    pub(crate) model_strength: f64,
    /// The chosen model directory, kept so a run does not re-ask.
    pub(crate) model_dir: Option<PathBuf>,
    /// What the directory says it is, for the panel.
    pub(crate) model_summary: Option<SharedString>,
    /// Loaded weights. Cached: reading them is the slow part.
    pub(crate) model_loaded: Option<std::rc::Rc<font_ml::outline::OutlineModel>>,
    /// Last judgement: glyph, model error, baseline error.
    pub(crate) model_score: Option<(SharedString, f64, f64)>,
    pub(crate) model_strength_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    pub(crate) status_note: Option<SharedString>,
    pub(crate) search: gpui::Entity<widgets::input::InputState>,
    pub(crate) search_query: String,
    /// Search scope: 0 = all, 1 = name, 2 = unicode.
    pub(crate) search_mode: u8,
    /// Wall-clock time of the last save, for the header.
    pub(crate) last_save_label: Option<SharedString>,
    /// Multi-selected glyph names (grid cmd/shift-click); `selected`
    /// stays the primary.
    pub(crate) multi_selected: std::collections::HashSet<String>,
    pub(crate) search_regex: bool,
    pub(crate) search_case: bool,
    pub(crate) metric_inputs: MetricInputs,
    pub(crate) font_info_inputs: FontInfoInputs,
    pub(crate) kern_inputs: KernInputs,
    /// Slant angle field in the Transformations section (degrees).
    pub(crate) slant_input: gpui::Entity<widgets::input::InputState>,
    /// Stroke width field in the Transformations section (units).
    pub(crate) stroke_input: gpui::Entity<widgets::input::InputState>,
    /// Offset field: bolder (positive) or lighter (negative) units.
    pub(crate) offset_input: gpui::Entity<widgets::input::InputState>,
    /// Fit Curve percentage field in the Curves section.
    pub(crate) fit_input: gpui::Entity<widgets::input::InputState>,
    /// Hex field that appends a color to the CPAL palette.
    pub(crate) color_hex_input: gpui::Entity<widgets::input::InputState>,
    /// Palette index the next color layer is assigned.
    pub(crate) color_selected: usize,
    /// Paint the color layers stacked in the editor.
    pub(crate) show_color_preview: bool,
    /// Which built-in sample string the buffer shows.
    pub(crate) sample_index: usize,
    /// Font view mode: the classic grid, the Glyphs 4 detail grid
    /// (info beside every glyph), or the property-table list.
    pub(crate) font_view_mode: FontViewMode,
    /// Draw node trajectories + velocity dots across the first axis
    /// (higher-order interpolation view).
    pub(crate) show_trajectories: bool,
    /// The intermediate point being dragged right now (id, Q),
    /// painted live and committed + baked on mouse-up.
    pub(crate) hoi_live: Option<((usize, usize), (f64, f64))>,
    /// The shaping inspector's focused cluster (carrier sort index).
    pub(crate) shaping_focus: Option<usize>,
    /// Ghost every attachable mark on the open glyph's anchors
    /// (Glyphs' mark cloud).
    pub(crate) show_mark_cloud: bool,
    /// Preview feature overrides: tag → forced on/off. Absent tags
    /// keep the shaper's defaults.
    pub(crate) feature_overrides: std::collections::HashMap<String, bool>,
    /// Preview shaping locale: (script tag, BCP 47 language), e.g.
    /// ("arab", "ur"). None = direction-derived defaults.
    pub(crate) shaping_locale: Option<(String, String)>,
    /// Ease amount field: Enter bakes interpolation timing into a
    /// brace layer at the preview location.
    pub(crate) ease_input: gpui::Entity<widgets::input::InputState>,
    /// Extrude field ("offset,angle"; k-prefix keeps the front).
    pub(crate) extrude_input: gpui::Entity<widgets::input::InputState>,
    /// Roughen field ("segment,h,v"); reseeded per apply.
    pub(crate) roughen_input: gpui::Entity<widgets::input::InputState>,
    pub(crate) roughen_seed: u64,
    /// The Instances editor field under the axis sliders: Enter
    /// renames the instance at the preview location, or adds one.
    pub(crate) instance_name_input: gpui::Entity<widgets::input::InputState>,
    /// The Features section's features.fea editor (grid mode).
    pub(crate) features_input: gpui::Entity<widgets::input::InputState>,
    /// Unapplied edits in the features editor: the refresh keeps its
    /// hands off until Apply or Revert.
    pub(crate) features_edited: bool,
    /// The last Apply's compile verdict, shown under the editor.
    pub(crate) features_status: Option<SharedString>,
    pub(crate) glyph_inputs: GlyphInputs,
    pub(crate) context_menu: Option<ContextMenu>,
    /// The Selection panel's 9-point reference for numeric move and
    /// scale (web coordinate quadrant).
    pub(crate) coord_quadrant: runebender_core::outline::path::Quadrant,
    /// Curve overlays (web CurvePanel).
    pub(crate) curve_comb: bool,
    pub(crate) curve_continuity: bool,
    /// Measure-tool HUD layers (web SelectPanel / MeasureOptions).
    pub(crate) measure_opts: MeasureOpts,
    /// Show the UFO background layer as a quiet outline.
    pub(crate) show_background: bool,
    /// Per-glyph UFO layers drawn as underlays (layer names with the
    /// eye on), beyond the default and background layers.
    pub(crate) visible_glyph_layers: std::collections::HashSet<String>,
    /// Another glyph ghosted behind the drawing for comparison.
    pub(crate) reference_glyph: Option<String>,
    pub(crate) reference_glyph_input: gpui::Entity<widgets::input::InputState>,
    pub(crate) component_name_input: gpui::Entity<widgets::input::InputState>,
    /// Corner-glyph name typed in the context menu (Apply Corner…).
    pub(crate) corner_name_input: gpui::Entity<widgets::input::InputState>,
    /// Note text typed in the context menu (Annotate: Note…).
    pub(crate) annotation_input: gpui::Entity<widgets::input::InputState>,
    /// Smart-axis definition on the open part glyph ("Width,0,100").
    pub(crate) smart_axis_input: gpui::Entity<widgets::input::InputState>,
    /// New kerning group from the grid selection: "o" (kern1) or
    /// "|o" (kern2).
    pub(crate) group_name_input: gpui::Entity<widgets::input::InputState>,
    /// New avar pair on the first axis: "user,design".
    pub(crate) axis_map_input: gpui::Entity<widgets::input::InputState>,
    /// Parsed predicate query, rebuilt when the search changes.
    pub(crate) search_predicates: Option<Vec<SearchPred>>,
    /// The selected smart component's value on its first axis.
    pub(crate) smart_value_input: gpui::Entity<widgets::input::InputState>,
    pub(crate) anchor_name_input: gpui::Entity<widgets::input::InputState>,
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
    pub(crate) last_save: Arc<Mutex<web_time::Instant>>,
    /// A selected kern pair in the preview strip: indices into the
    /// resolved preview line (glyph indices of the pair).
    pub(crate) _subscriptions: Vec<gpui::Subscription>,
}

/// The editor's Width / LSB / RSB / X / Y fields.
/// Which measurement-HUD layers the Measure tool draws (web
/// MeasureOptions). Every layer off returns the plain editor; the
/// panel is purely additive.
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
pub(crate) static MEASURE_MENU: std::sync::Mutex<MeasureOpts> =
    std::sync::Mutex::new(MeasureOpts {
        colorize: false,
        handles: false,
        segments: false,
        spans: false,
        sidebearings: false,
        sizes: false,
        popcount: true,
    });

impl MeasureOpts {
    pub(crate) fn any(&self) -> bool {
        self.colorize
            || self.handles
            || self.segments
            || self.spans
            || self.sidebearings
            || self.sizes
    }

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
    pub(crate) name: gpui::Entity<widgets::input::InputState>,
    pub(crate) unicode: gpui::Entity<widgets::input::InputState>,
    pub(crate) group_l: gpui::Entity<widgets::input::InputState>,
    pub(crate) group_r: gpui::Entity<widgets::input::InputState>,
    /// Free-text glyph note (UFO glif note element), like Glyphs'
    /// note field; shows as a tooltip in its font view.
    pub(crate) note: gpui::Entity<widgets::input::InputState>,
    /// Shape-switch point: Enter creates the .bold alternate and the
    /// designspace rule at this axis value (bracket layer).
    pub(crate) switch_at: gpui::Entity<widgets::input::InputState>,
    /// Metrics keys ("=n", "=|o", "=n+10"): linked sidebearings,
    /// synced across every master.
    pub(crate) lsb_key: gpui::Entity<widgets::input::InputState>,
    pub(crate) rsb_key: gpui::Entity<widgets::input::InputState>,
    /// Export (production) name, written to public.postscriptNames
    /// in every master's lib; ufo2ft renames on compile.
    pub(crate) production: gpui::Entity<widgets::input::InputState>,
}

pub(crate) struct MetricInputs {
    pub(crate) width: gpui::Entity<widgets::input::InputState>,
    pub(crate) lsb: gpui::Entity<widgets::input::InputState>,
    pub(crate) rsb: gpui::Entity<widgets::input::InputState>,
    /// Selection reference coordinates and size (Selection section).
    pub(crate) x: gpui::Entity<widgets::input::InputState>,
    pub(crate) y: gpui::Entity<widgets::input::InputState>,
    pub(crate) w: gpui::Entity<widgets::input::InputState>,
    pub(crate) h: gpui::Entity<widgets::input::InputState>,
}

/// Editable fields in the Font Info section (grid mode). Each commits
/// on Enter and writes fontinfo.plist through the normal save path.
pub(crate) struct FontInfoInputs {
    pub(crate) family: gpui::Entity<widgets::input::InputState>,
    pub(crate) style: gpui::Entity<widgets::input::InputState>,
    pub(crate) upm: gpui::Entity<widgets::input::InputState>,
    pub(crate) italic_angle: gpui::Entity<widgets::input::InputState>,
    pub(crate) ascender: gpui::Entity<widgets::input::InputState>,
    pub(crate) descender: gpui::Entity<widgets::input::InputState>,
    pub(crate) x_height: gpui::Entity<widgets::input::InputState>,
    pub(crate) cap_height: gpui::Entity<widgets::input::InputState>,
    /// PostScript hinting data per master: alignment zones (blue
    /// values in pairs) and standard stems, comma-separated lists.
    pub(crate) blue_values: gpui::Entity<widgets::input::InputState>,
    pub(crate) other_blues: gpui::Entity<widgets::input::InputState>,
    pub(crate) stems_h: gpui::Entity<widgets::input::InputState>,
    pub(crate) stems_v: gpui::Entity<widgets::input::InputState>,
    /// The OS/2 and hhea vertical metrics (typo/hhea/win), the
    /// parameters the Google Fonts vertical-metrics checks read.
    pub(crate) typo_asc: gpui::Entity<widgets::input::InputState>,
    pub(crate) typo_desc: gpui::Entity<widgets::input::InputState>,
    pub(crate) typo_gap: gpui::Entity<widgets::input::InputState>,
    pub(crate) hhea_asc: gpui::Entity<widgets::input::InputState>,
    pub(crate) hhea_desc: gpui::Entity<widgets::input::InputState>,
    pub(crate) hhea_gap: gpui::Entity<widgets::input::InputState>,
    pub(crate) win_asc: gpui::Entity<widgets::input::InputState>,
    pub(crate) win_desc: gpui::Entity<widgets::input::InputState>,
}

/// The Kerning section's inputs: a live filter over the pair list,
/// and a first/second/value editor that commits on Enter.
pub(crate) struct KernInputs {
    pub(crate) filter: gpui::Entity<widgets::input::InputState>,
    pub(crate) first: gpui::Entity<widgets::input::InputState>,
    pub(crate) second: gpui::Entity<widgets::input::InputState>,
    pub(crate) value: gpui::Entity<widgets::input::InputState>,
}

/// Which Font Info field an input commits to.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum FontInfoField {
    Family,
    Style,
    Upm,
    ItalicAngle,
    Ascender,
    Descender,
    XHeight,
    CapHeight,
    TypoAscender,
    TypoDescender,
    TypoLineGap,
    HheaAscender,
    HheaDescender,
    HheaLineGap,
    WinAscent,
    WinDescent,
    BlueValues,
    OtherBlues,
    StemsH,
    StemsV,
}

/// How a grid of glyph cells fits its pane: cell size, and how many
/// columns and whole rows are on screen.
#[derive(Clone, Copy)]
pub(crate) struct GridFit {
    pub(crate) cell_w: f32,
    pub(crate) cell_h: f32,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
}

impl GridFit {
    /// Exact width of a full row of cells, gaps included.
    pub(crate) fn content_w(&self) -> f32 {
        self.cell_w * self.cols as f32 + GRID_GAP * (self.cols - 1) as f32
    }
}

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

pub(crate) const ZOOM_MIN: f64 = 1e-3;

pub(crate) const ZOOM_MAX: f64 = 1e4;

/// One press of the zoom keys.
pub(crate) const ZOOM_KEY_STEP: f64 = 1.1;

/// Height of a header tab, and the side of the square icon buttons
/// that sit beside tabs in the header and the status bar.
pub(crate) const TAB_H: f32 = 24.0;

/// Gap between grid cells, and the grid's inner padding.
pub(crate) const GRID_GAP: f32 = 8.0;

pub(crate) const GRID_PAD: f32 = 12.0;

pub(crate) const GRID_PAD_Y: f32 = 8.0;

/// The sidebar's mini grid is narrow: it spares less padding, but the
/// fit is solved the same way.
pub(crate) const GRID_PAD_SM: f32 = 6.0;

pub(crate) const HIT_RADIUS_PX: f64 = 10.0;

/// Points are easier to grab than segments: the web select tool gives
/// them a wider radius (SELECT_POINT_HIT_DISTANCE) than the 10px it
/// uses for segments, metric edges and components.
pub(crate) const POINT_HIT_RADIUS_PX: f64 = 16.0;

/// The config file's contents, read once before the window opens.
///
/// A `OnceLock` rather than a re-read per call: the file is read at
/// startup and changing it means restarting, which is the same promise
/// the theme menu makes.
pub(crate) static CONFIG: std::sync::OnceLock<config::Config> = std::sync::OnceLock::new();
