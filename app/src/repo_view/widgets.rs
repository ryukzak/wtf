//! Small pieces of UI the views share.

use super::*;

/// A strip on a panel border that resizes the panel when dragged; a grip in
/// its middle shows it can be dragged.
pub(super) fn border_handle(id: &'static str, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w_full()
        .h(px(8.))
        .border_t_1()
        .border_b_1()
        .border_color(theme.separator)
        .bg(theme.graph_background)
        .hover(|strip| strip.bg(theme.selection))
        .cursor(CursorStyle::ResizeUpDown)
        .child(
            div()
                .w(px(28.))
                .h(px(3.))
                .rounded(px(2.))
                .bg(theme.secondary),
        )
}

/// The graph border's grip turned upright, for the side panel's left edge.
pub(super) fn side_border_handle(id: &'static str, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(8.))
        .h_full()
        .border_l_1()
        .border_r_1()
        .border_color(theme.separator)
        .bg(theme.graph_background)
        .hover(|strip| strip.bg(theme.selection))
        .cursor(CursorStyle::ResizeLeftRight)
        .group(id)
        // The grip shows on hover only: beside diff rows, a lone mark reads
        // as a fault in one of them.
        .child(
            div()
                .w(px(3.))
                .h(px(28.))
                .rounded(px(2.))
                .bg(theme.secondary)
                .invisible()
                .group_hover(id, |grip| grip.visible()),
        )
}

/// A section title in the commit list.
pub(super) fn list_heading(text: String, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .px(px(10.))
        .pt(px(8.))
        .pb(px(2.))
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.secondary)
        .child(text)
}

pub(super) fn list_item(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex()
        .flex_col()
        .flex_none()
        .justify_center()
        .w_full()
        .h(px(LIST_ITEM_HEIGHT))
        .px(px(10.))
        .overflow_hidden()
        .when(selected, |item| item.bg(theme.selection))
}

/// A list item for context around the list, dimmed, picked by a click.
pub(super) fn dim_list_item(id: impl Into<gpui::ElementId>, theme: &Theme) -> Stateful<gpui::Div> {
    list_item(id, false, theme)
        .text_color(theme.secondary)
        .cursor_pointer()
        .hover(|item| item.bg(theme.selection.opacity(0.5)))
}

pub(super) fn placeholder(title: &str, description: Option<String>, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .child(div().text_lg().child(SharedString::from(title.to_string())))
        .children(description.map(|text| div().text_color(theme.secondary).child(text)))
        .into_any_element()
}

pub(super) fn column(width: f32) -> gpui::Div {
    div().flex_none().w(px(width)).h_full()
}

/// A box of the side strip: a panel's button, or a group of modes.
pub(super) fn strip_group(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(6.))
        .border_1()
        .border_color(theme.separator)
}

/// A button in a box of the side strip, filled while on.
pub(super) fn strip_option(id: &'static str, on: bool, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .px(px(2.))
        .py(px(6.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|option| option.bg(theme.selection.opacity(0.5)))
        .when(on, |option| option.bg(theme.selection))
}

/// Text turned to read downwards, for the sidebar strip. GPUI draws text
/// level only, so it is an SVG image, drawn at twice its size to stay sharp.
pub(super) fn vertical_label(text: &str, color: gpui::Hsla, window: &Window) -> impl IntoElement {
    let size = f32::from(window.rem_size()) * 0.8;
    let family = match preferences::ui_font() {
        font if font.starts_with('.') => "Helvetica Neue".to_string(),
        font => format!("{font}, Helvetica Neue"),
    };
    let run = gpui::TextRun {
        len: text.len(),
        font: gpui::font(preferences::ui_font()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let length = window
        .text_system()
        .shape_line(text.to_string().into(), px(size), &[run], None)
        .width;
    let (w, h) = ((size * 1.4).ceil(), (f32::from(length) + 4.).ceil());
    let (fill, opacity) = svg_color(color);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {w} {h}"><text transform="translate({} {}) rotate(90)" text-anchor="middle" dy="0.35em" font-family="{family}" font-size="{size}" fill="{fill}" fill-opacity="{opacity}">{text}</text></svg>"#,
        w * 2.,
        h * 2.,
        w / 2.,
        h / 2.,
    );
    gpui::img(std::sync::Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(w))
    .h(px(h))
}

/// A fold chevron in the side panels: as large as their text, drawn a little
/// into the gap after it so the name sits close.
pub(super) fn panel_chevron(open: bool, theme: &Theme) -> impl IntoElement {
    div().flex_none().mr(px(-3.)).child(chevron(
        open,
        theme.secondary,
        (preferences::ui_size() * 0.875).round(),
    ))
}

/// A chevron pointing left or right, for the side strip's buttons.
pub(super) fn side_chevron(left: bool, color: gpui::Hsla, size: f32) -> impl IntoElement {
    let path = match left {
        true => "M10 3.5 L5.5 8 L10 12.5",
        false => "M6 3.5 L10.5 8 L6 12.5",
    };
    stroke_icon(path, color, size)
}

/// A line drawing on a 16-unit grid, as an image of `size` points.
/// A floating panel's look: a bordered card with a shadow.
pub(super) fn floating<E: Styled>(element: E, theme: &Theme) -> E {
    element
        .border_1()
        .border_color(theme.separator)
        .bg(theme.background)
        .shadow_md()
}

/// A button floating at the bottom of the graph, to place at a side.
pub(super) fn graph_button(
    id: &'static str,
    label: &'static str,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    floating(div().id(id), theme)
        .absolute()
        .bottom(px(10.))
        .px(px(8.))
        .py(px(2.))
        .rounded(px(4.))
        .text_xs()
        .cursor_pointer()
        .hover(|button| button.bg(theme.selection))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(label)
}

/// A color as SVG takes it: `rgb(…)` and an opacity.
fn svg_color(color: gpui::Hsla) -> (String, f32) {
    let rgba = gpui::Rgba::from(color);
    let byte = |channel: f32| (channel * 255.) as u8;
    let rgb = format!("rgb({},{},{})", byte(rgba.r), byte(rgba.g), byte(rgba.b));
    (rgb, rgba.a)
}

pub(super) fn stroke_icon(path: &str, color: gpui::Hsla, size: f32) -> impl IntoElement {
    let (stroke, opacity) = svg_color(color);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="2.5 2.5 11 11"><path d="{path}" fill="none" stroke="{stroke}" stroke-opacity="{opacity}" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"#,
    );
    gpui::img(std::sync::Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .flex_none()
    .w(px(size))
    .h(px(size))
}

/// An empty diff row under the commit header.
pub(super) fn spacer() -> AnyElement {
    div().h(px(diff_view::row_height())).into_any_element()
}

/// A fold chevron, `›` when folded and `⌄` when open, drawn as an image so
/// it is crisp at any size and the same in every font.
pub fn chevron(open: bool, color: gpui::Hsla, size: f32) -> impl IntoElement {
    let path = match open {
        true => "M3.5 6 L8 10.5 L12.5 6",
        false => "M6 3.5 L10.5 8 L6 12.5",
    };
    stroke_icon(path, color, size)
}
