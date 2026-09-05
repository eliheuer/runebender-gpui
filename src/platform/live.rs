// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Service core's live document mailbox on GPUI's entity thread.

use crate::Workspace;
use gpui::Context;
use runebender_core::document::{live, live_socket::Server};

impl Workspace {
    /// Gives each newly opened document its own endpoint; old clients disconnect.
    pub(crate) fn reset_live(&mut self) {
        self.live = None;
        self.live = Server::start()
            .map_err(|e| eprintln!("Live tools unavailable: {e}"))
            .ok();
        if let Some(server) = &self.live {
            eprintln!("Runebender live session: {}", server.path().display());
        }
    }

    /// Starts one mailbox pump for the window's lifetime.
    pub(crate) fn start_live_pump(&mut self, cx: &mut Context<'_, Self>) {
        self.reset_live();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(40))
                    .await;
                if this
                    .update(cx, |workspace, cx| {
                        let request = workspace.live.as_ref().and_then(Server::try_recv);
                        if let Some(request) = request {
                            request.respond(|call| match workspace.project.as_mut() {
                                Some(project) => live::call(project, &call.name, &call.arguments),
                                None => {
                                    serde_json::json!({"ok": false, "error": "no document open"})
                                }
                            });
                            workspace.refresh_proposal();
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
}
