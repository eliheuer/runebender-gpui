// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Reload when the sources change underneath us.
//!
//! A font project is edited by more than this program: a build script
//! writes a master, another editor saves, a git checkout moves the
//! tree. The editor follows what happens on disk rather than assuming
//! it is the only writer.
//!
//! runebender-xilem has the same file, doing the same job through its
//! own framework.

use gpui::Context;

use crate::Workspace;

impl Workspace {
    /// Watch every master's UFO directory.
    ///
    /// External changes reload the affected masters. In-memory edits
    /// are never clobbered: dirty masters skip the reload with a
    /// status note. Our own saves are suppressed via the `last_save`
    /// timestamp.
    #[cfg(target_family = "wasm")]
    pub(crate) fn start_watching(&mut self, _cx: &mut Context<'_, Self>) {
        // No filesystem on the web: live reload will ride the host
        // data layer instead.
    }

    #[cfg(not(target_family = "wasm"))]
    /// The native arm: watch each master's source directory with `notify` and reload masters that change on disk.
    pub(crate) fn start_watching(&mut self, cx: &mut Context<'_, Self>) {
        use futures::StreamExt;
        self._watcher = None;
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let mut watcher =
            match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = tx.unbounded_send(());
                }
            }) {
                Ok(w) => w,
                Err(_) => return,
            };
        for master in &project.masters {
            let _ = notify::Watcher::watch(
                &mut watcher,
                &master.source_path,
                notify::RecursiveMode::Recursive,
            );
        }
        self._watcher = Some(watcher);
        let last_save = self.last_save.clone();
        cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                // Debounce: drain everything arriving in the next
                // half second into one reload.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                while rx.try_recv().is_ok() {}
                if last_save.lock().expect("the last-save lock").elapsed()
                    < std::time::Duration::from_secs(2)
                {
                    continue;
                }
                if this
                    .update(cx, |workspace, cx| {
                        workspace.reload_from_disk();
                        cx.notify();
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
