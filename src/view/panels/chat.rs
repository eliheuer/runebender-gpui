// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Chat pane: a transcript, a prompt, and the model to use.

use crate::Workspace;
use crate::edit::chat::ChatEntry;
use crate::view::controls as c;
use crate::view::theme as t;
use crate::widgets;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::div;
use gpui::prelude::FluentBuilder;
use gpui::px;

impl Workspace {
    /// The pane. The model choice sits at the top, the transcript
    /// fills the middle, the prompt is pinned at the bottom.
    pub(crate) fn chat_panel(&self, cx: &mut Context<'_, Self>) -> gpui::Div {
        let mut body = c::column().size_full().min_h(px(0.0));

        // Which model. GGUF folders under the model roots.
        if self.chat.installed.is_empty() {
            body = body.child(div().text_color(t::text_muted()).child(
                "No chat model. Put a folder holding a .gguf and its tokenizer.json \
                 under ~/.runebender/models. Qwen3 4B is the one this was built with.",
            ));
        } else {
            let mut row = c::row().flex_wrap();
            for (name, path) in self.chat.installed.clone() {
                let on = self.chat.model.as_deref() == Some(path.as_path());
                row = row.child(
                    c::toggle(SharedString::from(format!("chat-model-{name}")), name, on)
                        .flex_none()
                        .px_2()
                        .w_auto()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.chat.model = Some(path.clone());
                            cx.notify();
                        })),
                );
            }
            body = body.child(row);
        }

        // The transcript.
        let mut log = div()
            .id("chat-log")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        for (i, entry) in self.chat.entries.iter().enumerate() {
            log = log.child(match entry {
                ChatEntry::User(text) => div()
                    .id(("chat-user", i))
                    .px_2()
                    .py_1()
                    .rounded(t::radius())
                    .bg(t::selected_bg())
                    .text_color(t::selected_ink())
                    .child(text.clone()),
                ChatEntry::Assistant(text) => div()
                    .id(("chat-assistant", i))
                    .px_1()
                    .py_1()
                    .text_color(t::text())
                    .child(if text.is_empty() {
                        SharedString::from("…")
                    } else {
                        text.clone()
                    }),
                ChatEntry::Tool { name, ok, note } => div()
                    .id(("chat-tool", i))
                    .px_2()
                    .py_0p5()
                    .rounded(t::radius())
                    .border(t::stroke())
                    .border_color(t::cell_border())
                    .text_color(t::text_muted())
                    .child(format!("{} {name}: {note}", if *ok { "→" } else { "✗" })),
                ChatEntry::Error(text) => div()
                    .id(("chat-error", i))
                    .px_1()
                    .py_1()
                    .text_color(t::text())
                    .child(format!("Error: {text}")),
            });
        }
        body = body.child(log);

        // Status and speed, one line.
        let status: Option<SharedString> = self
            .chat
            .busy
            .clone()
            .or_else(|| self.chat.last_speed.clone());
        if let Some(s) = status {
            body = body.child(div().text_color(t::text_muted()).child(s));
        }

        // The prompt, Send, Cancel, Clear.
        let busy = self.chat.busy.is_some();
        body.child(
            c::row()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::input::Input::new(&self.chat_input)),
                )
                .child(
                    c::toggle("chat-send", if busy { "…" } else { "Send" }, !busy)
                        .flex_none()
                        .w(px(64.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let text = this.chat_input.read(cx).value().to_string();
                            this.chat_input
                                .update(cx, |state, cx| state.set_value("", window, cx));
                            this.chat_send(text, cx);
                            cx.notify();
                        })),
                )
                .when(busy, |el| {
                    el.child(
                        c::button("chat-cancel", "Cancel")
                            .flex_none()
                            .w(px(72.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.chat_cancel();
                                cx.notify();
                            })),
                    )
                })
                .when(!busy && !self.chat.entries.is_empty(), |el| {
                    el.child(
                        c::button("chat-clear", "Clear")
                            .flex_none()
                            .w(px(64.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.chat_clear();
                                cx.notify();
                            })),
                    )
                }),
        )
    }
}
