//! A text field as drawn: its text with the selection marked, a caret that
//! takes no room, drawn over the text, and the mouse placing the caret, a
//! drag or ⇧-click selecting, a double click selecting a word and a triple
//! click all of it. Editing by keys is [`LineEdit`]'s; up and down in a
//! multi-line field go by the text as laid out, wrapped lines too.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, Context, CursorStyle, HighlightStyle, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, StyledText, TextLayout, canvas, div, point, prelude::*, px,
};

use crate::line_edit::LineEdit;
use crate::theme::Theme;

#[derive(Clone, Default)]
pub struct Field {
    pub edit: LineEdit,
    /// The text as last laid out, to place the caret by the mouse and move
    /// it between lines.
    layout: Rc<RefCell<Option<TextLayout>>>,
    /// A press in the text is dragging out a selection.
    dragging: bool,
}

impl Field {
    pub fn new(edit: LineEdit) -> Self {
        Self {
            edit,
            ..Self::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.edit.text
    }

    /// Handles a key, as [`LineEdit::key`] does, and up and down between the
    /// lines of a multi-line field. Returns whether the key was the field's.
    pub fn key(&mut self, keystroke: &Keystroke, cx: &mut gpui::App) -> bool {
        let m = keystroke.modifiers;
        let ctrl = m.control && !m.platform && !m.alt;
        let down = match keystroke.key.as_str() {
            "up" if !m.platform && !m.alt => false,
            "down" if !m.platform && !m.alt => true,
            "p" if ctrl => false,
            "n" if ctrl => true,
            _ => return self.edit.key(keystroke, cx),
        };
        if !self.edit.multiline {
            return self.edit.key(keystroke, cx);
        }
        let to = self.line_target(down);
        self.edit.go(to, m.shift);
        true
    }

    /// Where the caret lands a line up or `down`, by where the text lies on
    /// screen: the start or end of the text past its first or last line.
    fn line_target(&self, down: bool) -> usize {
        let edge = if down { self.edit.text.len() } else { 0 };
        let Some(layout) = self.layout.borrow().clone() else {
            return edge;
        };
        let Some(at) = layout.position_for_index(self.edit.caret) else {
            return edge;
        };
        let line = layout.line_height();
        let target = point(
            at.x + px(1.),
            at.y + if down { line * 1.5 } else { -line * 0.5 },
        );
        let bounds = layout.bounds();
        if target.y < bounds.top() || target.y > bounds.bottom() {
            return edge;
        }
        layout.index_for_position(target).unwrap_or_else(|i| i)
    }

    /// The place in the text under the mouse.
    fn index_at(&self, position: gpui::Point<gpui::Pixels>) -> Option<usize> {
        let layout = self.layout.borrow().clone()?;
        Some(layout.index_for_position(position).unwrap_or_else(|i| i))
    }

    /// The field drawn, `placeholder` while it is empty. The caret shows
    /// while `focused` and `caret_on`, for one that blinks; `styles` are
    /// highlights of the field's own, under the selection. A press in it
    /// gives it `focus`; `field` finds it in the view, for the mouse to move
    /// its caret.
    #[allow(clippy::too_many_arguments)]
    pub fn render<V: 'static>(
        &self,
        id: impl Into<gpui::ElementId>,
        placeholder: &str,
        focused: bool,
        caret_on: bool,
        styles: Vec<(Range<usize>, HighlightStyle)>,
        focus: &gpui::FocusHandle,
        theme: &Theme,
        field: impl Fn(&mut V) -> Option<&mut Field> + Copy + 'static,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let caret_color = theme.accent;
        let text = &self.edit.text;
        let shown: AnyElement = if text.is_empty() {
            self.layout.replace(None);
            // Over a placeholder the caret sits at its start, where typing
            // goes.
            div()
                .relative()
                .text_color(theme.secondary)
                .child(placeholder.to_string())
                .when(focused && caret_on, |field| {
                    field.child(
                        div()
                            .absolute()
                            .left(px(-0.75))
                            .top_0()
                            .bottom_0()
                            .w(px(1.5))
                            .bg(caret_color),
                    )
                })
                .into_any_element()
        } else {
            let range = self.edit.selection();
            let selection = (focused && !range.is_empty()).then(|| {
                (
                    range,
                    HighlightStyle {
                        background_color: Some(theme.selection),
                        ..Default::default()
                    },
                )
            });
            let highlights: Vec<_> = gpui::combine_highlights(styles, selection).collect();
            let styled = StyledText::new(text.clone()).with_highlights(highlights);
            let layout = styled.layout().clone();
            self.layout.replace(Some(layout.clone()));
            let caret = self.edit.caret;
            div()
                .relative()
                .child(styled)
                .when(focused && caret_on, |shown| {
                    shown.child(
                        canvas(
                            |_, _, _| (),
                            move |_, _, window, _| {
                                let Some(at) = layout.position_for_index(caret) else {
                                    return;
                                };
                                let bounds = gpui::Bounds::new(
                                    point(at.x - px(0.75), at.y),
                                    gpui::size(px(1.5), layout.line_height()),
                                );
                                window.paint_quad(gpui::fill(bounds, caret_color));
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                })
                .into_any_element()
        };
        let focus = focus.clone();
        div()
            .id(id)
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    window.focus(&focus);
                    let Some(field) = field(view) else {
                        return;
                    };
                    let at = field.index_at(event.position);
                    match (event.click_count, at) {
                        (_, None) => field.edit.go(field.edit.text.len(), event.modifiers.shift),
                        (1, Some(at)) => field.edit.go(at, event.modifiers.shift),
                        (2, Some(at)) => field.edit.select_word(at),
                        (_, Some(_)) => {
                            field.edit.anchor = Some(0);
                            field.edit.caret = field.edit.text.len();
                        }
                    }
                    field.dragging = event.click_count == 1;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
                let Some(field) = field(view) else {
                    return;
                };
                if !field.dragging || event.pressed_button != Some(MouseButton::Left) {
                    field.dragging = false;
                    return;
                }
                if let Some(at) = field.index_at(event.position) {
                    field.edit.go(at, true);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, _, _, _| {
                    if let Some(field) = field(view) {
                        field.dragging = false;
                    }
                }),
            )
            .child(shown)
            .into_any_element()
    }
}
