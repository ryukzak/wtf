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

/// Where the commit list's text starts, and inside a bar of several
/// commits, with room for its chevron.
pub(super) const LIST_PADDING: f32 = 10.;
pub(super) const LIST_INDENT: f32 = 18.;

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
        .pl(px(LIST_PADDING))
        .pr(px(10.))
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

/// A list that scrolls only up and down: GPUI otherwise turns a wheel
/// moved only sideways, as a trackball's or a mouse's tilt is, into a
/// scroll down the list.
pub(super) fn vertical_only<S: Styled>(mut element: S) -> S {
    element.style().restrict_scroll_to_axis = Some(true);
    element
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

/// How tall a [`vertical_label`] of `text` draws.
pub(super) fn vertical_label_length(text: &str, window: &Window) -> f32 {
    let size = f32::from(window.rem_size()) * 0.8;
    let run = gpui::TextRun {
        len: text.len(),
        font: gpui::font(preferences::ui_font()),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let length = window
        .text_system()
        .shape_line(text.to_string().into(), px(size), &[run], None)
        .width;
    (f32::from(length) + 4.).ceil()
}

/// How tall a box of the side strip is around buttons labelled `labels`,
/// each `extra` taller than its label, as [`strip_group`] and
/// [`strip_option`] lay them out.
pub(super) fn strip_group_height(labels: &[&str], extra: f32, window: &Window) -> f32 {
    let options: f32 = labels
        .iter()
        .map(|label| vertical_label_length(label, window) + 12. + extra)
        .sum();
    options + 2. * labels.len().saturating_sub(1) as f32 + 4. + 2.
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

/// How the mark and waiting text move while the model works: over and
/// over.
pub(super) fn working_animation() -> Animation {
    Animation::new(Duration::from_millis(1200)).repeat()
}

/// The mark of Apple Intelligence's work, dots on a circle as macOS draws
/// it working: with `lead`, the place of the dot ahead, 0 to 1 round the
/// circle, the dots trail off behind it, smaller and dimmer; without, they
/// rest alike.
pub(super) fn ai_mark(size: f32, lead: Option<f32>, theme: &Theme) -> impl IntoElement + use<> {
    dots(size, lead, theme.foreground, theme.secondary)
}

/// [`ai_mark`]'s dots, from `dim` to `bright` by how near the lead they are.
fn dots(size: f32, lead: Option<f32>, bright: gpui::Hsla, dim: gpui::Hsla) -> impl IntoElement {
    const DOTS: usize = 8;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let round = size * 0.34;
            for k in 0..DOTS {
                let place = k as f32 / DOTS as f32;
                // How near the lead the dot is, 1 at it, falling off behind.
                let near = match lead {
                    Some(lead) => 1. - (lead - place).rem_euclid(1.),
                    None => 0.4,
                };
                let angle = std::f32::consts::TAU * place - std::f32::consts::FRAC_PI_2;
                let at = center + point(px(round * angle.cos()), px(round * angle.sin()));
                let dot = |radius: f32, color: gpui::Hsla, window: &mut Window| {
                    let bounds = gpui::Bounds::new(
                        at - point(px(radius), px(radius)),
                        gpui::size(px(2. * radius), px(2. * radius)),
                    );
                    window.paint_quad(gpui::fill(bounds, color).corner_radii(px(radius)));
                };
                let radius = size * (0.05 + 0.05 * near);
                // A glow round the brightest.
                if lead.is_some() && near > 0.7 {
                    dot(radius * 2.2, bright.opacity(0.18 * near), window);
                }
                dot(radius, mix(dim.opacity(0.5), bright, near), window);
            }
        },
    )
    .flex_none()
    .size(px(size))
}

/// [`ai_mark`] while the model works, its dots going round.
pub(super) fn working_mark(
    id: impl Into<gpui::ElementId>,
    size: f32,
    theme: &Theme,
) -> gpui::AnimationElement<gpui::Div> {
    let (bright, dim) = (theme.foreground, theme.secondary);
    div()
        .flex_none()
        .with_animation(id, working_animation(), move |mark, delta| {
            mark.child(dots(size, Some(delta), bright, dim))
        })
}

/// `from` blended toward `to` by `amount`, 0 to 1, in RGB.
fn mix(from: gpui::Hsla, to: gpui::Hsla, amount: f32) -> gpui::Hsla {
    let (from, to) = (gpui::Rgba::from(from), gpui::Rgba::from(to));
    let channel = |a: f32, b: f32| a + (b - a) * amount;
    gpui::Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a: channel(from.a, to.a),
    }
    .into()
}

/// Text waiting on the model, such as "Writing a commit message…": a
/// light band sweeps across it, from `base` up to `bright` and back.
pub(super) fn shimmer(
    id: impl Into<gpui::ElementId>,
    text: String,
    base: gpui::Hsla,
    bright: gpui::Hsla,
) -> impl IntoElement {
    /// How many characters the band lights on each side of its middle.
    const BAND: f32 = 6.;
    div().with_animation(
        id,
        Animation::new(Duration::from_millis(2000)).repeat(),
        move |row, delta| {
            let count = text.chars().count() as f32;
            // The band comes in from before the first character and leaves
            // past the last, so the text rests a moment between sweeps.
            let middle = -BAND + delta * (count + 2. * BAND);
            let highlights = text.char_indices().enumerate().map(|(n, (at, c))| {
                let light = (1. - (n as f32 - middle).abs() / BAND).max(0.);
                (
                    at..at + c.len_utf8(),
                    HighlightStyle {
                        color: Some(mix(base, bright, light)),
                        ..Default::default()
                    },
                )
            });
            row.child(StyledText::new(text.clone()).with_highlights(highlights.collect::<Vec<_>>()))
        },
    )
}
