// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Live node actions and background proof images. Graph geometry is shared with core.

/// Native process-backed experiment controls.
#[cfg(unix)]
mod native {
    use crate::Workspace;
    use gpui::Context;
    use runebender_core::document::nodes_live as live;
    use serde_json::{Value, json};
    use std::{collections::BTreeMap, sync::Arc};

    /// Completed proof images; each retains the scene that produced it.
    #[derive(Default)]
    pub(crate) struct ExperimentPreviews {
        /// Node key to rendered scene and image.
        pub(crate) images: BTreeMap<String, (Value, Arc<gpui::RenderImage>, String)>,
        /// One background render at a time bounds CPU and memory use.
        busy: bool,
        /// Remaining proof nodes for an explicit graph run, scoped to its file.
        queue: std::collections::VecDeque<(std::path::PathBuf, u32)>,
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

        /// Advance an explicit graph run without allowing concurrent renderer processes.
        fn next_live_proof(&mut self, cx: &mut Context<'_, Self>) {
            while let Some((path, id)) = self.models.experiment_previews.queue.pop_front() {
                if self.models.graph.as_ref().is_some_and(|g| g.path == path) {
                    self.preview_experiment(id, false, false, cx);
                    if self.models.experiment_previews.busy {
                        break;
                    }
                }
            }
        }

        /// Run live forks in connection order and render their proofs, without applying outputs.
        pub(crate) fn run_live_nodes(&mut self, cx: &mut Context<'_, Self>) {
            if self.models.experiment_previews.busy {
                self.status_note = Some("A proof is rendering".into());
                return;
            }
            let (Some(state), Some(project)) = (self.models.graph.as_mut(), self.project.as_mut())
            else {
                return;
            };
            if state
                .graph
                .nodes
                .iter()
                .any(|n| !n.type_name.starts_with("live."))
            {
                self.status_note=Some("Disk task nodes require a separate workflow; live fonts cannot be passed to them yet".into());
                return;
            }
            if let Some(problem) = state.graph.validate(&state.registry).first() {
                self.status_note = Some(problem.to_string().into());
                return;
            }
            let order = match state.graph.order() {
                Ok(v) => v,
                Err(_) => {
                    self.status_note = Some("Remove the cycle before running".into());
                    return;
                }
            };
            for id in order {
                if state
                    .graph
                    .node(id)
                    .is_some_and(|n| n.type_name == "live.fork")
                    && let Err(e) = live::create_version(&mut state.graph, project, id)
                {
                    self.status_note = Some(e.into());
                    return;
                }
            }
            self.models.experiment_previews.queue = state
                .graph
                .nodes
                .iter()
                .filter(|n| n.type_name == "live.proof")
                .map(|n| (state.path.clone(), n.id))
                .collect();
            self.status_note = Some(
                "Versions ready; rendering proofs. Apply outputs run only when clicked.".into(),
            );
            self.next_live_proof(cx);
        }

        /// Render a snapshot on a worker and attach the exact rendered image to its node.
        fn preview_experiment(
            &mut self,
            id: u32,
            text: bool,
            latest: bool,
            cx: &mut Context<'_, Self>,
        ) {
            if self.models.experiment_previews.busy {
                self.status_note = Some("A proof is rendering; wait for it to finish".into());
                return;
            }
            let selection = self.selection_names();
            let Some(graph) = self.models.graph.as_ref() else {
                return;
            };
            let Some(project) = self.project.as_mut() else {
                return;
            };
            let version = match live::resolve(&graph.graph, project, id) {
                Ok(v) => v,
                Err(e) => {
                    self.status_note = Some(e.into());
                    return;
                }
            };
            let branch = version.branch;
            let caption = format!(
                "Snapshot · {} · master {}",
                branch.as_deref().unwrap_or("root"),
                version.master
            );
            let proof_key = format!("{}:{}", version.master, branch.as_deref().unwrap_or("root"));
            let key = format!("{}:{id}", graph.path.display());
            let result = if latest {
                json!({"scene":project.experiments.proofs.get(&proof_key),"error":"Ask OMP for a proof first"})
            } else {
                let mut args = json!({"master":version.master});
                if let Some(branch) = branch {
                    args["branch"] = json!(branch);
                }
                if text {
                    args["text"] = json!("AVATAR To Wa");
                } else {
                    let font = if let Some(name) = args["branch"].as_str() {
                        &project.experiments.versions[name].master.font
                    } else {
                        &project.masters[version.master].font
                    };
                    let names: Vec<_> = font
                        .default_layer()
                        .iter()
                        .filter(|g| !g.contours.is_empty() || !g.components.is_empty())
                        .take(6)
                        .map(|g| g.name().to_string())
                        .collect();
                    args["glyphs"] = json!(if selection.is_empty() {
                        names
                    } else {
                        selection.iter().take(256).cloned().collect()
                    });
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
                        this.models.experiment_previews.images.insert(key,(scene,image,caption));
                        this.status_note=Some("Snapshot proof ready. Refresh after edits; Export PDF uses this exact snapshot.".into());
                    }
                    Err(e)=>this.status_note=Some(e.into()),
                }
                this.next_live_proof(cx);
                cx.notify();
            });
        }).detach();
        }

        /// Export the displayed snapshot through the platform save dialog.
        fn export_experiment_proof(&mut self, key: &str, pdf: bool, cx: &mut Context<'_, Self>) {
            let Some((scene, _, _)) = self.models.experiment_previews.images.get(key) else {
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

        /// Hit-test actions using the same scaled rectangles as the canvas painter.
        pub(crate) fn live_node_mouse_down(
            &mut self,
            pos: gpui::Point<gpui::Pixels>,
            cx: &mut Context<'_, Self>,
        ) -> bool {
            let Some(state) = self.models.graph.as_ref() else {
                return false;
            };
            // Let an open context menu own its clicks.
            if self.models.graph_view.menu.is_some() {
                return false;
            }
            let view = &self.models.graph_view;
            let origin = view.bounds.lock().unwrap_or_else(|e| e.into_inner()).origin;
            let at = runebender_core::ui::nodes::to_canvas(
                &view.viewport,
                kurbo::Point::new(
                    f64::from(f32::from(pos.x - origin.x)),
                    f64::from(f32::from(pos.y - origin.y)),
                ),
            );
            let boxes = runebender_core::ui::nodes::layout(&state.graph, &state.registry);
            let runebender_core::ui::nodes::Hit::Node(id) =
                runebender_core::ui::nodes::hit(&boxes, at)
            else {
                return false;
            };
            let Some(node) = boxes.iter().find(|n| n.id == id) else {
                return false;
            };
            for (i, action) in runebender_core::ui::nodes::actions(&node.type_name)
                .iter()
                .enumerate()
            {
                if node.action_rect(i).contains(at) {
                    self.models.graph_view.selected = Some(id);
                    self.models.graph_view.drag = None;
                    self.live_node_action(id, action, cx);
                    return true;
                }
            }
            false
        }

        /// Dispatch a compact action inside a live graph node.
        pub(crate) fn live_node_action(
            &mut self,
            id: u32,
            action: &str,
            cx: &mut Context<'_, Self>,
        ) {
            if self.editor.drag.is_some() {
                return;
            }
            match action {
                "Render glyphs" | "Render kerning" | "Latest OMP proof" => {
                    self.preview_experiment(
                        id,
                        action == "Render kerning",
                        action == "Latest OMP proof",
                        cx,
                    );
                    return;
                }
                "Export PDF…" | "Export PNG…" => {
                    if let Some(g) = self.models.graph.as_ref() {
                        let key = format!("{}:{id}", g.path.display());
                        if !self.models.experiment_previews.images.contains_key(&key) {
                            self.status_note = Some("Render this proof before exporting".into());
                            return;
                        }
                        self.export_experiment_proof(&key, action == "Export PDF…", cx);
                    }
                    return;
                }
                "Undo last application" => {
                    self.experiment_command("experiment_undo_apply", json!({}));
                    return;
                }
                "Save as new UFO…" => {
                    self.save_node_font(id, cx);
                    return;
                }
                _ => {}
            }
            let (Some(state), Some(project)) = (self.models.graph.as_mut(), self.project.as_mut())
            else {
                return;
            };
            let result: Result<String, String> = (|| match action {
                "Create version" => {
                    let v = live::create_version(&mut state.graph, project, id)?;
                    Ok(format!(
                        "OMP: edit branch {} on master {}. The root is unchanged.",
                        v.branch.unwrap_or_default(),
                        v.master
                    ))
                }
                "Fork direction" => {
                    live::resolve(&state.graph, project, id)?;
                    let n = state.graph.node(id).ok_or("missing node")?;
                    let pos = [
                        n.pos[0] + 304.0,
                        state
                            .graph
                            .nodes
                            .iter()
                            .map(|n| n.pos[1])
                            .fold(0.0_f32, f32::max)
                            + 384.0,
                    ];
                    live::add_direction(&mut state.graph, id, pos);
                    Ok(
                        "Connected a new direction. Create its version to snapshot the input."
                            .into(),
                    )
                }
                "Add apply node" => {
                    live::resolve(&state.graph, project, id)?;
                    let n = state.graph.node(id).ok_or("missing node")?;
                    let pos = [n.pos[0] + 608.0, n.pos[1]];
                    let output = state.graph.add("live.apply", pos);
                    state.graph.connect(id, "font", output, "font");
                    Ok("Connected an explicit Apply output".into())
                }
                "Apply changes" => {
                    let result = live::apply(&state.graph, project, id)?;
                    Ok(result.to_string())
                }
                "Discard version" => {
                    let v = live::resolve(&state.graph, project, id)?;
                    live::discard(project, &v.branch.ok_or("Cannot discard the root")?)?;
                    state.graph.remove(id);
                    Ok("Discarded version; root unchanged".into())
                }
                _ => Err("Unknown node action".into()),
            })();
            self.status_note = Some(result.unwrap_or_else(|e| e).into());
            self.nodes_revalidate();
            if action == "Apply changes" {
                self.editor.selected.clear();
                self.editor.selected_anchors.clear();
                self.editor.selected_component = None;
                self.editor.hyper_contour = None;
                self.rebuild_text_models();
            }
            cx.notify();
        }

        /// Export a captured master as a new UFO without changing the loaded document.
        fn save_node_font(&mut self, id: u32, cx: &mut Context<'_, Self>) {
            let (Some(state), Some(project)) = (self.models.graph.as_ref(), self.project.as_ref())
            else {
                return;
            };
            let version = match live::resolve(&state.graph, project, id) {
                Ok(v) => v,
                Err(e) => {
                    self.status_note = Some(e.into());
                    return;
                }
            };
            let Some(branch) = version.branch else {
                return;
            };
            let font = project.experiments.versions[&branch].master.font.clone();
            let filename = format!("{branch}.ufo");
            let dialog = cx.prompt_for_new_path(&std::env::temp_dir(), Some(&filename));
            cx.spawn(async move |this, cx| {
                let Ok(Ok(Some(path))) = dialog.await else {
                    return;
                };
                let result =
                    cx.background_executor()
                        .spawn(async move {
                            runebender_core::document::nodes_live::save_new(&font, &path)
                        })
                        .await;
                let _ = this.update(cx, |this, cx| {
                    this.status_note = Some(
                        result
                            .map(|()| "Saved a new UFO; loaded source unchanged".to_string())
                            .unwrap_or_else(|e| e)
                            .into(),
                    );
                    cx.notify();
                });
            })
            .detach();
        }
    }
}

#[cfg(unix)]
pub(crate) use native::ExperimentPreviews;

/// Browser builds do not hold native renderer previews.
#[cfg(not(unix))]
#[derive(Default)]
pub(crate) struct ExperimentPreviews;
