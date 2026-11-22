use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{Hsla, WindowAppearance, hsla};

/// The appearance the user chose in Preferences: follow the system, or
/// always light or dark.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Appearance {
    System,
    Light,
    Dark,
}

static APPEARANCE: AtomicU8 = AtomicU8::new(0);

impl Appearance {
    pub fn current() -> Self {
        match APPEARANCE.load(Ordering::Relaxed) {
            1 => Self::Light,
            2 => Self::Dark,
            _ => Self::System,
        }
    }

    pub fn set(self) {
        let value = match self {
            Self::System => 0,
            Self::Light => 1,
            Self::Dark => 2,
        };
        APPEARANCE.store(value, Ordering::Relaxed);
    }
}

/// Sublime Merge's colors: diffs from Breakers (light) and Mariana (dark), the
/// graph from the Merge and Merge Dark themes.
pub struct Theme {
    pub background: Hsla,
    pub foreground: Hsla,
    pub secondary: Hsla,
    pub separator: Hsla,
    pub selection: Hsla,
    pub filler: Hsla,

    pub accent: Hsla,
    pub error: Hsla,

    pub graph_background: Hsla,
    /// Lane colors, used in turn.
    pub lanes: [Hsla; 6],
    pub tag_background: Hsla,
    /// Text on tags filled with a lane color.
    pub tag_head_text: Hsla,
    /// Git tags, yellow as in Sublime Merge.
    pub git_tag_background: Hsla,
    pub git_tag_text: Hsla,

    pub changed_text: Hsla,
    pub changed_char_text: Hsla,

    pub deleted: Hsla,
    pub deleted_char: Hsla,
    pub inserted: Hsla,
    pub inserted_char: Hsla,
}

impl Theme {
    /// The theme for a window, unless Preferences fix the appearance.
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match (Appearance::current(), appearance) {
            (Appearance::Light, _) => Self::light(),
            (Appearance::Dark, _) => Self::dark(),
            (_, WindowAppearance::Light | WindowAppearance::VibrantLight) => Self::light(),
            (_, WindowAppearance::Dark | WindowAppearance::VibrantDark) => Self::dark(),
        }
    }

    fn light() -> Self {
        Self {
            background: hsl(180., 9., 99., 1.),
            foreground: hsl(0., 0., 20., 1.),
            secondary: hsl(0., 0., 60., 1.),
            separator: hsl(0., 0., 0., 0.1),
            selection: hsl(211., 100., 50., 0.15),
            filler: hsl(0., 0., 20., 0.05),
            accent: hsl(210., 60., 55., 1.),
            error: hsl(357., 60., 45., 1.),
            graph_background: hsl(210., 10., 93., 1.),
            lanes: [
                hsl(210., 100., 60., 1.),
                hsl(260., 80., 65., 1.),
                hsl(330., 80., 60., 1.),
                hsl(32., 100., 50., 1.),
                hsl(114., 50., 50., 1.),
                hsl(180., 100., 35., 1.),
            ],
            tag_background: hsl(210., 30., 91., 1.),
            tag_head_text: hsl(0., 0., 100., 1.),
            git_tag_background: hsl(44., 100., 60., 1.),
            git_tag_text: hsl(44., 100., 20., 1.),
            changed_text: hsl(0., 0., 0., 1.),
            changed_char_text: hsl(0., 0., 0., 1.),
            deleted: hsl(0., 65., 89., 1.),
            deleted_char: hsl(0., 70., 75., 1.),
            inserted: hsl(114., 55., 87., 1.),
            inserted_char: hsl(114., 55., 70., 1.),
        }
    }

    fn dark() -> Self {
        Self {
            background: hsl(210., 15., 22., 1.),
            foreground: hsl(219., 28., 88., 1.),
            secondary: hsl(221., 12., 69., 1.),
            separator: hsl(0., 0., 100., 0.1),
            selection: hsl(211., 100., 60., 0.25),
            filler: hsl(219., 28., 88., 0.05),
            accent: hsl(210., 60., 60., 1.),
            error: hsl(357., 79., 65., 1.),
            graph_background: hsl(210., 10., 19., 1.),
            lanes: [
                hsl(210., 65., 70., 1.),
                hsl(260., 60., 75., 1.),
                hsl(330., 65., 65., 1.),
                hsl(0., 70., 70., 1.),
                hsl(44., 75., 60., 1.),
                hsl(180., 60., 65., 1.),
            ],
            tag_background: hsl(210., 10., 13., 1.),
            tag_head_text: hsl(0., 0., 0., 1.),
            git_tag_background: hsl(44., 100., 60., 1.),
            git_tag_text: hsl(210., 10., 10., 1.),
            changed_text: hsl(219., 28., 93., 1.),
            changed_char_text: hsl(219., 28., 98., 1.),
            deleted: hsl(357., 50., 60., 0.22),
            deleted_char: hsl(357., 65., 60., 0.45),
            inserted: hsl(180., 50., 60., 0.22),
            inserted_char: hsl(180., 65., 60., 0.45),
        }
    }
}

/// Takes CSS-style units: degrees and percents.
fn hsl(hue: f32, saturation: f32, lightness: f32, alpha: f32) -> Hsla {
    hsla(hue / 360., saturation / 100., lightness / 100., alpha)
}
