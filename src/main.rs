// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Runebender GPUI: a font editor built on [GPUI](https://gpui.rs/),
//! started as a point of comparison against
//! [runebender-xilem](https://github.com/eliheuer/runebender-xilem).

use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

struct Workspace {
    status: SharedString,
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .justify_center()
            .items_center()
            .gap_2()
            .bg(rgb(0x28211c))
            .text_color(rgb(0xe8ddcf))
            .child(div().text_xl().child("Runebender GPUI"))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa89a86))
                    .child(self.status.clone()),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1024.), px(768.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Runebender".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Workspace {
                    status: "No font loaded".into(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
