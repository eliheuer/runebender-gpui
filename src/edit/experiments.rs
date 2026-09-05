// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Live experiment cards in the node workspace; font state and rendering live in core.

/// Native process-backed experiment controls.
#[cfg(unix)]
mod native {
    use crate::Workspace;
    use crate::view::{controls as c, theme as t};
    use gpui::{Context, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px};
    use gpui::{InteractiveElement as _, StyledImage as _};
    use serde_json::{Value, json};
    use std::{collections::BTreeMap, sync::Arc};

    /// Completed proof images; each retains the scene that produced it.
    #[derive(Default)]
    pub(crate) struct ExperimentPreviews {
        /// Node key to rendered scene and image.
        images: BTreeMap<String, (Value, Arc<gpui::RenderImage>)>,
        /// One background render at a time bounds CPU and memory use.
        busy: bool,
    }

    impl Workspace {
        /// Execute a shared live command, refreshing root UI only after a root change.
        fn experiment_command(&mut self, name: &str, args: Value) {
            let Some(project) = self.project.as_mut() else {
                return;
            };
            let result = runebender_core::document::live::call(project, name, &args);
            self.status_note = Some(result.to_string().into());
            if result["root_changed"] == true {
                self.editor.selected.clear();
                self.editor.selected_anchors.clear();
                self.editor.selected_component = None;
                self.editor.hyper_contour = None;
                self.rebuild_text_models();
            }
        }

        /// Fork a root or experiment into the next available session name.
        fn fork_experiment(&mut self, parent: Option<&str>) {
            let Some(project) = self.project.as_ref() else {
                return;
            };
            let mut n = 1;
            while project
                .experiments
                .versions
                .contains_key(&format!("version-{n}"))
            {
                n += 1;
            }
            let mut args = json!({"master":project.active,"name":format!("version-{n}"),"reason":"Node workspace experiment"});
            if let Some(parent) = parent {
                args["parent"] = json!(parent);
            }
            self.experiment_command("experiment_fork", args);
        }

        /// Render a snapshot on a worker and attach the exact rendered image to its card.
        fn preview_experiment(
            &mut self,
            branch: Option<String>,
            text: bool,
            latest: bool,
            cx: &mut Context<'_, Self>,
        ) {
            if self.models.experiment_previews.busy {
                return;
            }
            let Some(project) = self.project.as_mut() else {
                return;
            };
            let key = format!("{}:{}", project.active, branch.as_deref().unwrap_or("root"));
            let result = if latest {
                json!({"scene":project.experiments.proofs.get(&key),"error":"Ask OMP for a proof first"})
            } else {
                let mut args = json!({"master":project.active});
                if let Some(branch) = branch {
                    args["branch"] = json!(branch);
                }
                if text {
                    args["text"] = json!("AVATAR To Wa");
                } else {
                    let font = if let Some(name) = args["branch"].as_str() {
                        &project.experiments.versions[name].master.font
                    } else {
                        &project.active_font().font
                    };
                    let names: Vec<_> = font
                        .default_layer()
                        .iter()
                        .filter(|g| !g.contours.is_empty() || !g.components.is_empty())
                        .take(6)
                        .map(|g| g.name().to_string())
                        .collect();
                    args["glyphs"] = json!(names);
                }
                runebender_core::document::live::call(
                    project,
                    if text { "specimen" } else { "proof" },
                    &args,
                )
            };
            let Some(scene) = result.get("scene").filter(|v| !v.is_null()).cloned() else {
                self.status_note = Some(result.to_string().into());
                return;
            };
            let document = self.live.as_ref().map(|s| s.path().to_path_buf());
            self.models.experiment_previews.busy = true;
            cx.spawn(async move |this,cx| {
            let render_scene=scene.clone();
            let result=cx.background_executor().spawn(async move {runebender_core::formats::designbot::render(&render_scene,false)}).await;
            let _=this.update(cx,|this,cx| {
                if document!=this.live.as_ref().map(|s|s.path().to_path_buf()) {return;}
                this.models.experiment_previews.busy=false;
                match result.and_then(|bytes| image::load_from_memory(&bytes).map_err(|e|e.to_string())) {
                    Ok(image)=>{
                        let mut buffer=image.to_rgba8();
                        for p in buffer.pixels_mut() {p.0.swap(0,2);}
                        let image=Arc::new(gpui::RenderImage::new(vec![image::Frame::new(buffer)]));
                        this.models.experiment_previews.images.insert(key,(scene,image));
                        this.status_note=Some("Snapshot proof ready. Refresh after edits; Export PDF uses this exact snapshot.".into());
                    }
                    Err(e)=>this.status_note=Some(e.into()),
                }
                cx.notify();
            });
        }).detach();
        }

        /// Export the displayed snapshot through the platform save dialog.
        fn export_experiment_proof(&mut self, key: &str, pdf: bool, cx: &mut Context<'_, Self>) {
            let Some((scene, _)) = self.models.experiment_previews.images.get(key) else {
                return;
            };
            let scene = scene.clone();
            let dialog = cx.prompt_for_new_path(
                &std::env::temp_dir(),
                Some(if pdf {
                    "runebender-proof.pdf"
                } else {
                    "runebender-proof.png"
                }),
            );
            cx.spawn(async move |this, cx| {
                let Ok(Ok(Some(path))) = dialog.await else {
                    return;
                };
                let output = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let bytes = runebender_core::formats::designbot::render(&scene, pdf)?;
                        std::fs::write(output, bytes).map_err(|e| e.to_string())
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.status_note = Some(
                        match result {
                            Ok(()) => format!("Saved {}", path.display()),
                            Err(e) => e,
                        }
                        .into(),
                    );
                    cx.notify();
                });
            })
            .detach();
        }

        /// A compact live-version graph above the general node canvas.
        pub(crate) fn experiment_nodes(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
            let Some(project) = self.project.as_ref() else {
                return div();
            };
            let active = project.active;
            let mut nodes = vec![(None, "Root (live)".to_string())];
            nodes.extend(
                project
                    .experiments
                    .versions
                    .iter()
                    .filter(|(_, v)| v.root == active)
                    .map(|(name, v)| {
                        (
                            Some(name.clone()),
                            format!("{} → {name}", v.parent.as_deref().unwrap_or("Root")),
                        )
                    }),
            );
            let mut row = div()
                .flex()
                .gap_2()
                .p_2()
                .h(px(380.0))
                .border_b_1()
                .border_color(t::panel_outline());
            for (branch, title) in nodes {
                let key = format!("{active}:{}", branch.as_deref().unwrap_or("root"));
                let fork = branch.clone();
                let glyph = branch.clone();
                let text = branch.clone();
                let mut card = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .w(px(220.0))
                    .flex_shrink_0()
                    .border_1()
                    .border_color(t::panel_outline())
                    .child(title)
                    .child(
                        c::button(gpui::SharedString::from(format!("fork-{key}")), "Fork")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.fork_experiment(fork.as_deref());
                                cx.notify();
                            })),
                    )
                    .child(
                        c::button(
                            gpui::SharedString::from(format!("glyph-{key}")),
                            "Refresh glyph proof",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.preview_experiment(glyph.clone(), false, false, cx);
                            cx.notify();
                        })),
                    )
                    .child(
                        c::button(
                            gpui::SharedString::from(format!("text-{key}")),
                            "Refresh kerning proof",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.preview_experiment(text.clone(), true, false, cx);
                            cx.notify();
                        })),
                    );
                if project.experiments.proofs.contains_key(&key) {
                    let latest = branch.clone();
                    card = card.child(
                        c::button(
                            gpui::SharedString::from(format!("latest-{key}")),
                            "Show latest agent proof",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.preview_experiment(latest.clone(), false, true, cx);
                            cx.notify();
                        })),
                    );
                }
                if let Some((_, image)) = self.models.experiment_previews.images.get(&key) {
                    card = card.child(
                        gpui::img(image.clone())
                            .w(px(200.0))
                            .h(px(100.0))
                            .object_fit(gpui::ObjectFit::Contain),
                    );
                    let export = key.clone();
                    card = card.child("Snapshot · refresh after edits").child(
                        c::button(
                            gpui::SharedString::from(format!("pdf-{key}")),
                            "Export snapshot PDF",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.export_experiment_proof(&export, true, cx);
                        })),
                    );
                }
                if self.models.experiment_previews.images.contains_key(&key) {
                    let export = key.clone();
                    card = card.child(
                        c::button(
                            gpui::SharedString::from(format!("png-{key}")),
                            "Export snapshot PNG",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.export_experiment_proof(&export, false, cx);
                        })),
                    );
                }
                if let Some(branch) = branch {
                    card=card.child(c::button(gpui::SharedString::from(format!("apply-{key}")),"Apply all · redraw allowed").on_click(cx.listener(move |this,_,_,cx| {
                    if this.editor.drag.is_some() {return;}
                    let Some(project)=this.project.as_ref() else {return;};
                    let v=&project.experiments.versions[&branch];
                    let names:Vec<_>=v.master.font.default_layer().iter().filter(|g|v.base.get_glyph(g.name().as_str())!=Some(*g)).map(|g|g.name().to_string()).collect();
                    this.experiment_command("experiment_apply",json!({"master":active,"branch":branch,"glyphs":names,"kerning":true,"keep_structure":false}));cx.notify();
                })));
                } else {
                    card = card.child("Versions last until this font closes.").child(
                        c::button("undo-version", "Undo last application").on_click(cx.listener(
                            |this, _, _, cx| {
                                if this.editor.drag.is_none() {
                                    this.experiment_command("experiment_undo_apply", json!({}));
                                }
                                cx.notify();
                            },
                        )),
                    );
                }
                row = row.child(card);
            }
            div().child(row.id("live-experiment-cards").overflow_x_scroll())
        }
    }
}

#[cfg(unix)]
pub(crate) use native::ExperimentPreviews;

/// Browser builds do not hold native renderer previews.
#[cfg(not(unix))]
#[derive(Default)]
pub(crate) struct ExperimentPreviews;

#[cfg(not(unix))]
impl crate::Workspace {
    /// Explain the native-only experiment bridge in unsupported hosts.
    pub(crate) fn experiment_nodes(&self, _: &mut gpui::Context<'_, Self>) -> gpui::Div {
        use gpui::ParentElement as _;
        gpui::div().child("Live experiment previews require the native macOS or Linux editor.")
    }
}
