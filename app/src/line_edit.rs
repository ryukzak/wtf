//! A one-line text field's text, caret and selection, edited with the keys
//! macOS text fields take: arrows, with ⌥ by word and ⌘ to the ends, ⇧ to
//! select, ⌫ and ⌦, ⌘A, and the clipboard.

use gpui::{ClipboardItem, Keystroke};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineEdit {
    pub text: String,
    /// The caret, as a byte offset on a char boundary.
    pub caret: usize,
    /// The other end of a selection, when there is one.
    pub anchor: Option<usize>,
}

/// How far a move goes.
#[derive(Clone, Copy)]
enum Reach {
    Char,
    Word,
    End,
}

impl LineEdit {
    /// The text with the caret at its end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            caret: text.len(),
            text,
            anchor: None,
        }
    }

    /// The selected bytes, in order; empty at the caret when none.
    pub fn selection(&self) -> std::ops::Range<usize> {
        let anchor = self.anchor.unwrap_or(self.caret);
        anchor.min(self.caret)..anchor.max(self.caret)
    }

    /// Handles a key, with `clipboard` read and written through `cx`.
    /// Returns whether the key was the field's.
    pub fn key(&mut self, keystroke: &Keystroke, cx: &mut gpui::App) -> bool {
        let m = keystroke.modifiers;
        let reach = |word: bool, end: bool| match (end, word) {
            (true, _) => Reach::End,
            (_, true) => Reach::Word,
            _ => Reach::Char,
        };
        match keystroke.key.as_str() {
            "left" => self.step(false, reach(m.alt, m.platform), m.shift),
            "right" => self.step(true, reach(m.alt, m.platform), m.shift),
            "home" | "up" => self.step(false, Reach::End, m.shift),
            "end" | "down" => self.step(true, Reach::End, m.shift),
            "a" if m.control => self.step(false, Reach::End, m.shift),
            "e" if m.control => self.step(true, Reach::End, m.shift),
            "b" if m.control => self.step(false, Reach::Char, m.shift),
            "f" if m.control => self.step(true, Reach::Char, m.shift),
            "backspace" => self.delete(false, reach(m.alt, m.platform)),
            "delete" => self.delete(true, reach(m.alt, m.platform)),
            "h" if m.control => self.delete(false, Reach::Char),
            "d" if m.control => self.delete(true, Reach::Char),
            "k" if m.control => self.delete(true, Reach::End),
            "a" if m.platform => {
                self.anchor = Some(0);
                self.caret = self.text.len();
            }
            "c" | "x" if m.platform => {
                let range = self.selection();
                if range.is_empty() {
                    return true;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(self.text[range].to_string()));
                if keystroke.key == "x" {
                    self.replace("");
                }
            }
            "v" if m.platform => {
                if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.replace(pasted.replace(['\n', '\r'], " ").trim());
                }
            }
            _ if !m.platform && !m.control => match &keystroke.key_char {
                Some(typed) if !typed.chars().any(char::is_control) => self.replace(typed),
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    /// Puts `text` in place of the selection, or at the caret.
    pub fn replace(&mut self, text: &str) {
        let range = self.selection();
        self.text.replace_range(range.clone(), text);
        self.caret = range.start + text.len();
        self.anchor = None;
    }

    /// Moves the caret, selecting on the way with `select`. Without it, a
    /// selection collapses to the side the move goes to.
    fn step(&mut self, forward: bool, reach: Reach, select: bool) {
        let range = self.selection();
        if !select && !range.is_empty() && matches!(reach, Reach::Char) {
            self.caret = if forward { range.end } else { range.start };
            self.anchor = None;
            return;
        }
        let to = self.target(self.caret, forward, reach);
        match select {
            true => {
                self.anchor.get_or_insert(self.caret);
            }
            false => self.anchor = None,
        }
        self.caret = to;
        if self.anchor == Some(self.caret) {
            self.anchor = None;
        }
    }

    /// Deletes the selection, or from the caret as far as `reach`.
    fn delete(&mut self, forward: bool, reach: Reach) {
        if self.selection().is_empty() {
            let to = self.target(self.caret, forward, reach);
            self.anchor = Some(to);
        }
        self.replace("");
    }

    /// Where a move from `at` lands.
    fn target(&self, at: usize, forward: bool, reach: Reach) -> usize {
        let text = &self.text;
        match (reach, forward) {
            (Reach::End, true) => text.len(),
            (Reach::End, false) => 0,
            (Reach::Char, true) => text[at..].chars().next().map_or(at, |c| at + c.len_utf8()),
            (Reach::Char, false) => text[..at]
                .chars()
                .next_back()
                .map_or(at, |c| at - c.len_utf8()),
            // Past any separators, then to the end of the word, as macOS
            // fields move by word.
            (Reach::Word, true) => {
                let rest = &text[at..];
                let start = rest.find(is_word).unwrap_or(rest.len());
                let end = rest[start..]
                    .find(|c: char| !is_word(c))
                    .map_or(rest.len(), |end| start + end);
                at + end
            }
            (Reach::Word, false) => {
                let before = &text[..at];
                let end = before.rfind(is_word).map_or(0, |i| {
                    i + before[i..].chars().next().map_or(0, char::len_utf8)
                });
                before[..end].rfind(|c: char| !is_word(c)).map_or(0, |i| {
                    i + before[i..].chars().next().map_or(0, char::len_utf8)
                })
            }
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, caret: usize) -> LineEdit {
        LineEdit {
            text: text.to_string(),
            caret,
            anchor: None,
        }
    }

    #[test]
    fn typing_goes_in_at_the_caret_and_over_a_selection() {
        let mut edit = at("v[09]", 3);
        edit.replace("-");
        assert_eq!((edit.text.as_str(), edit.caret), ("v[0-9]", 4));
        edit.anchor = Some(0);
        edit.replace("x");
        assert_eq!((edit.text.as_str(), edit.caret), ("x9]", 1));
    }

    #[test]
    fn arrows_move_by_char_word_and_to_the_ends() {
        let mut edit = at("release/v2410-fix", 17);
        edit.step(false, Reach::Word, false);
        assert_eq!(edit.caret, 14, "before fix");
        edit.step(false, Reach::Word, false);
        assert_eq!(edit.caret, 8, "before v2410");
        edit.step(true, Reach::Word, false);
        assert_eq!(edit.caret, 13, "after v2410");
        edit.step(false, Reach::Char, false);
        assert_eq!(edit.caret, 12);
        edit.step(false, Reach::End, false);
        assert_eq!(edit.caret, 0);
        edit.step(true, Reach::End, false);
        assert_eq!(edit.caret, 17);
    }

    #[test]
    fn shift_selects_and_an_arrow_collapses_it() {
        let mut edit = at("stable", 6);
        edit.step(false, Reach::Char, true);
        edit.step(false, Reach::Char, true);
        assert_eq!(edit.selection(), 4..6);
        edit.step(false, Reach::Char, false);
        assert_eq!((edit.caret, edit.anchor), (4, None));
        // Selecting back to where it started selects nothing.
        edit.step(true, Reach::Char, true);
        edit.step(false, Reach::Char, true);
        assert_eq!(edit.anchor, None);
    }

    #[test]
    fn deleting_takes_a_char_a_word_or_the_selection() {
        let mut edit = at("*-lts", 5);
        edit.delete(false, Reach::Word);
        assert_eq!((edit.text.as_str(), edit.caret), ("*-", 2));
        edit.delete(false, Reach::Char);
        assert_eq!(edit.text, "*");
        let mut edit = at("v2410", 0);
        edit.delete(true, Reach::Char);
        assert_eq!((edit.text.as_str(), edit.caret), ("2410", 0));
        edit.delete(true, Reach::End);
        assert_eq!(edit.text, "");
    }

    #[test]
    fn moves_keep_to_char_boundaries() {
        let mut edit = at("чу", 4);
        edit.step(false, Reach::Char, false);
        assert_eq!(edit.caret, 2);
        edit.delete(false, Reach::Char);
        assert_eq!((edit.text.as_str(), edit.caret), ("у", 0));
    }
}
