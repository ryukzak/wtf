use gpui::{
    App, Bounds, Context, FocusHandle, Global, KeyDownEvent, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, div, prelude::*, px, size,
};

use crate::preferences;
use crate::theme::Theme;
use crate::workspace::{CloseTab, CloseWindow};

/// The open About window, so a second About brings it forward.
struct AboutWindow(Option<WindowHandle<About>>);

impl Global for AboutWindow {}

pub fn open(cx: &mut App) {
    let existing = cx.try_global::<AboutWindow>().and_then(|window| window.0);
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(360.), px(240.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("About wtf".into()),
            ..Default::default()
        }),
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            About { focus }
        })
    });
    if let Ok(handle) = opened {
        cx.set_global(AboutWindow(Some(handle)));
    }
}

pub struct About {
    focus: FocusHandle,
}

impl Render for About {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(preferences::ui_size()));
        let theme = Theme::for_appearance(window.appearance());
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            }))
            .on_action(|_: &CloseTab, window, _| window.remove_window())
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .size_full()
            .p(px(24.))
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(preferences::ui_font())
            .text_sm()
            .child(
                div()
                    .text_3xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .font_family(preferences::code_font())
                    .child("wtf"),
            )
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("WorkTree Flow"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.secondary)
                    .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .mt(px(10.))
                    .text_center()
                    .child("A native macOS Git client for reviewing AI agents' work."),
            )
            .child(
                div()
                    .text_center()
                    .text_color(theme.secondary)
                    .child("Also what you say when you open an agent's branch."),
            )
    }
}
