// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Images: tracing, placing, importing SVG.

use crate::Mode;
use crate::Workspace;
use gpui::Context;
use runebender_core::formats::svg::svg_to_contours;
impl Workspace {
    /// Glyph → Trace Image…: pick an image, autotrace it through
    /// img2bez (the web editor's tracer), and replace the current
    /// glyph's contours with the result. Undoable.
    pub(crate) fn command_trace_image(&mut self, cx: &mut Context<Self>) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Trace".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let bytes = std::fs::read(&path);
            this.update(cx, |workspace, cx| {
                match bytes {
                    Ok(bytes) => workspace.apply_image_trace(index, &bytes),
                    Err(e) => {
                        workspace.status_note = Some(format!("Trace: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Glyph > Place Image…: copy a picture into the UFO's images
    /// store and set it as this glyph's background image, scaled to
    /// the em and sitting on the descender. The tracing-template
    /// workflow; norad round-trips the images directory.
    pub(crate) fn command_place_image(&mut self, cx: &mut Context<Self>) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Place".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let bytes = std::fs::read(&path);
            this.update(cx, |workspace, cx| {
                match bytes {
                    Ok(bytes) => workspace.apply_place_image(index, &path, bytes),
                    Err(e) => {
                        workspace.status_note = Some(format!("Place image: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Glyph > Import SVG…: parse the file's path outlines and add
    /// them to the open glyph, fitted between descender and
    /// ascender, appended so existing drawing survives (undoable).
    pub(crate) fn command_import_svg(&mut self, cx: &mut Context<Self>) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let text = std::fs::read_to_string(&path);
            this.update(cx, |workspace, cx| {
                let (ascender, descender) = match workspace.font() {
                    Some(f) => (f.ascender, f.descender),
                    None => return,
                };
                match text
                    .map_err(|e| format!("{e}"))
                    .and_then(|t| svg_to_contours(&t, ascender, descender))
                {
                    Ok(contours) => {
                        workspace.push_undo_snapshot(index);
                        let added = contours.len();
                        let ok = workspace
                            .font_mut()
                            .and_then(|f| {
                                f.edit_glyph(index, |g| {
                                    g.contours.extend(contours);
                                    true
                                })
                            })
                            .unwrap_or(false);
                        if ok {
                            workspace.status_note =
                                Some(format!("Imported {added} SVG contour(s)").into());
                        } else {
                            workspace.editor.undo.pop();
                        }
                    }
                    Err(e) => {
                        workspace.status_note = Some(format!("SVG import: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Glyph > Remove Image: unlink this glyph's background image.
    /// The stored file stays; other glyphs may reference it.
    pub(crate) fn command_remove_image(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(font) = self.font_mut()
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str())
            && glyph.image.take().is_some()
        {
            font.dirty = true;
            font.modified_glyphs.insert(name);
        }
    }
}
