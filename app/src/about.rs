use gpui::{
    App, Context, FocusHandle, Global, KeyDownEvent, Window, WindowHandle, WindowOptions, div,
    prelude::*, px,
};

use crate::preferences;
use crate::theme::Theme;
use crate::workspace::{CloseTab, CloseWindow};

/// The app's icon, drawn as the Dock's is.
const ICON: &[u8] = include_bytes!("../icon/icon.svg");

/// Where the project lives, as Cargo.toml says.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// The open About window, so a second About brings it forward.
struct AboutWindow(Option<WindowHandle<About>>);

impl Global for AboutWindow {}

pub fn open(cx: &mut App) {
    let existing = cx.try_global::<AboutWindow>().and_then(|window| window.0);
    if preferences::bring_forward(existing, cx) {
        return;
    }
    let options = WindowOptions {
        is_minimizable: false,
        ..preferences::panel_options("About wtf", 360., 400., cx)
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
            // The app's icon, from the same drawing the Dock's is made of.
            .child(
                gpui::img(std::sync::Arc::new(gpui::Image::from_bytes(
                    gpui::ImageFormat::Svg,
                    ICON.to_vec(),
                )))
                .size(px(128.)),
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
            .child(
                div()
                    .id("about-project")
                    .mt(px(10.))
                    .text_color(theme.accent)
                    .cursor_pointer()
                    .hover(|link| link.underline())
                    .on_click(|_, _, cx| cx.open_url(REPOSITORY))
                    .child(REPOSITORY.trim_start_matches("https://")),
            )
    }
}
