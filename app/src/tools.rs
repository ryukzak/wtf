//! The External Tools window: whether the programs wtf runs but does not
//! ship are here and work, and how to fix those that are not.

use gpui::{
    App, ClipboardItem, Context, FocusHandle, FontWeight, Global, KeyDownEvent, SharedString,
    Window, WindowHandle, WindowOptions, div, prelude::*, px,
};
use wtf_core::ToolCheck;

use crate::preferences;
use crate::theme::Theme;
use crate::workspace::{CloseTab, CloseWindow};

/// The open tools window, so opening it again brings it forward.
struct ToolsWindow(Option<WindowHandle<Tools>>);

impl Global for ToolsWindow {}

/// At launch: checks the tools in the background, and opens the window when
/// any has a problem, unless the user asked not to be shown it again. Also
/// settles, on the first run, whether commit summaries start on.
pub fn check_at_launch(cx: &mut App) {
    let task = cx
        .background_executor()
        .spawn(async { wtf_core::check_tools() });
    cx.spawn(async move |cx| {
        let checks = task.await;
        cx.update(|cx| {
            preferences::decide_summaries(fm_ready(&checks), cx);
            if preferences::tool_check() && checks.iter().any(|check| !check.ok) {
                show(Some(checks), cx);
            }
        })
        .ok();
    })
    .detach();
}

/// Opens the window from the menu, checking again.
pub fn open(cx: &mut App) {
    show(None, cx);
}

/// Opens the window, or brings it forward, with checks already made or
/// with fresh ones.
fn show(checks: Option<Vec<ToolCheck>>, cx: &mut App) {
    let existing = cx.try_global::<ToolsWindow>().and_then(|window| window.0);
    if let Some(handle) = existing {
        let updated = handle.update(cx, |tools, window, cx| {
            window.activate_window();
            match checks.clone() {
                Some(checks) => tools.checks = Some(checks),
                None => tools.check(cx),
            }
            cx.notify();
        });
        if updated.is_ok() {
            return;
        }
    }
    let options = WindowOptions {
        is_minimizable: false,
        ..preferences::panel_options("External Tools", 560., 460., cx)
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            let mut tools = Tools {
                focus,
                checks: None,
                copied: None,
            };
            match checks {
                Some(checks) => tools.checks = Some(checks),
                None => tools.check(cx),
            }
            tools
        })
    });
    if let Ok(handle) = opened {
        cx.set_global(ToolsWindow(Some(handle)));
    }
}

fn fm_ready(checks: &[ToolCheck]) -> bool {
    checks.iter().any(|check| check.name == "fm" && check.ok)
}

pub struct Tools {
    focus: FocusHandle,
    /// `None` while checking.
    checks: Option<Vec<ToolCheck>>,
    /// The tool whose fix was last copied, to say so.
    copied: Option<&'static str>,
}

impl Tools {
    fn check(&mut self, cx: &mut Context<Self>) {
        self.checks = None;
        self.copied = None;
        let task = cx
            .background_executor()
            .spawn(async { wtf_core::check_tools() });
        cx.spawn(async move |this, cx| {
            let checks = task.await;
            this.update(cx, |this, cx| {
                this.checks = Some(checks);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn tool_row(&self, check: &ToolCheck, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let name = check.name;
        let (mark, color) = match check.ok {
            true => ("✓", theme.inserted_char),
            false => ("✕", theme.error),
        };
        let fix = check.fix.clone().map(|fix| {
            let copied = self.copied == Some(name);
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .mt(px(4.))
                .child(
                    div()
                        .flex_1()
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(4.))
                        .bg(theme.filler)
                        .font_family(preferences::code_font())
                        .font_weight(preferences::code_weight())
                        .child(fix.clone()),
                )
                .child(
                    button(SharedString::from(format!("copy-{name}")), theme)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(fix.clone()));
                            this.copied = Some(name);
                            cx.notify();
                        }))
                        .child(if copied { "Copied" } else { "Copy" }),
                )
        });
        // Summaries stay as the first run left them, so a model that
        // became ready since then needs them turned on.
        let summaries_off = name == "fm" && check.ok && !preferences::summaries();
        div()
            .flex()
            .gap(px(10.))
            .py(px(8.))
            .border_b_1()
            .border_color(theme.separator)
            .child(div().w(px(14.)).text_color(color).child(mark))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(check.purpose),
                    )
                    .child(
                        div()
                            .text_xs()
                            .when(!check.ok, |detail| detail.text_color(theme.error))
                            .child(check.detail.clone()),
                    )
                    .children(
                        check
                            .advice
                            .clone()
                            .map(|advice| div().text_xs().child(advice)),
                    )
                    .children(fix)
                    .when(summaries_off, |column| {
                        column.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_xs()
                                .child("Commit summaries are off in Settings.")
                                .child(
                                    button("summaries-on", theme)
                                        .on_click(|_, _, cx| preferences::set_summaries(true, cx))
                                        .child("Turn On"),
                                ),
                        )
                    }),
            )
    }
}

fn button(id: impl Into<gpui::ElementId>, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex_none()
        .px(px(10.))
        .py(px(2.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme.separator)
        .hover(|button| button.bg(theme.selection))
        .cursor_pointer()
}

impl Render for Tools {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(preferences::ui_size()));
        let theme = Theme::for_appearance(window.appearance());
        let intro = match &self.checks {
            None => "Checking…",
            Some(checks) if checks.iter().all(|check| check.ok) => {
                "Every program wtf relies on is here and works."
            }
            Some(_) => "Some programs wtf relies on are missing or do not work.",
        };
        let rows: Vec<gpui::Div> = self
            .checks
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|check| self.tool_row(check, &theme, cx))
            .collect();
        let hide = !preferences::tool_check();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                if matches!(event.keystroke.key.as_str(), "escape" | "enter") {
                    window.remove_window();
                }
            }))
            .on_action(|_: &CloseTab, window, _| window.remove_window())
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .flex()
            .flex_col()
            .size_full()
            .p(px(20.))
            .gap(px(8.))
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(preferences::ui_font())
            .text_sm()
            .child(div().child(intro))
            .child(
                div()
                    .id("tools")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .border_t_1()
                    .border_color(theme.separator)
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("hide-at-launch")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .on_click(move |_, _, cx| preferences::set_tool_check(hide, cx))
                            .child(
                                div()
                                    .text_color(if hide { theme.accent } else { theme.secondary })
                                    .child(if hide { "☑" } else { "☐" }),
                            )
                            .child("Don't show again at launch"),
                    )
                    .child(div().flex_1())
                    .child(
                        button("check-again", &theme)
                            .on_click(cx.listener(|this, _, _, cx| this.check(cx)))
                            .child("Check Again"),
                    )
                    .child(
                        // The default button, filled as Commit is; the hover
                        // dims it rather than greying it out.
                        div()
                            .id("ok")
                            .flex_none()
                            .px(px(14.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme.accent)
                            .bg(theme.accent)
                            .text_color(theme.tag_head_text)
                            .hover(|button| button.opacity(0.85))
                            .cursor_pointer()
                            .on_click(|_, window, _| window.remove_window())
                            .child("OK"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.secondary)
                    .child("This window is always in wtf › Check External Tools…"),
            )
    }
}
