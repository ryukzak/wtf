//! A text field's text, caret and selection, edited with the keys macOS
//! text fields take: arrows, with ⌥ by word and ⌘ to the line's ends, ⇧ to
//! select, ⌫ and ⌦ by character, word or line, ⌘A and the clipboard, and
//! the ⌃ keys of Emacs that macOS keeps (⌃A ⌃E ⌃B ⌃F ⌃H ⌃D ⌃K ⌃T ⌃O). A
//! multi-line field takes Return as a line break; moving up and down a line
//! needs the text as laid out, so the field's view does that.

use gpui::{ClipboardItem, Keystroke};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineEdit {
    pub text: String,
    /// The caret, as a byte offset on a char boundary.
    pub caret: usize,
    /// The other end of a selection, when there is one.
    pub anchor: Option<usize>,
    /// Return breaks the line, and pasted text keeps its lines.
    pub multiline: bool,
}

/// How far a move goes.
#[derive(Clone, Copy)]
enum Reach {
    Char,
    Word,
    /// To the start or end of the line.
    Line,
}

impl LineEdit {
    /// One line of text with the caret at its end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            caret: text.len(),
            text,
            anchor: None,
            multiline: false,
        }
    }

    /// Text of several lines with the caret at its end.
    pub fn multiline(text: impl Into<String>) -> Self {
        Self {
            multiline: true,
            ..Self::new(text)
        }
    }

    /// Puts in other text, the caret at its end, as when the field is
    /// filled from elsewhere.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.caret = self.text.len();
        self.anchor = None;
    }

    /// The selected bytes, in order; empty at the caret when none.
    pub fn selection(&self) -> std::ops::Range<usize> {
        let caret = self.caret.min(self.text.len());
        let anchor = self
            .anchor
            .filter(|&anchor| anchor <= self.text.len() && self.text.is_char_boundary(anchor))
            .unwrap_or(caret);
        anchor.min(caret)..anchor.max(caret)
    }

    /// Handles a key, with the clipboard read and written through `cx`.
    /// Returns whether the key was the field's.
    pub fn key(&mut self, keystroke: &Keystroke, cx: &mut gpui::App) -> bool {
        let m = keystroke.modifiers;
        match keystroke.key.as_str() {
            "c" | "x" if m.platform => {
                let range = self.selection();
                if !range.is_empty() {
                    let copied = self.text[range].to_string();
                    cx.write_to_clipboard(ClipboardItem::new_string(copied));
                    if keystroke.key == "x" {
                        self.replace("");
                    }
                }
                true
            }
            _ => self.edit(keystroke, || {
                cx.read_from_clipboard().and_then(|item| item.text())
            }),
        }
    }

    /// Handles a key other than copying, with `paste` giving what ⌘V puts
    /// in. Returns whether the key was the field's: up and down in a
    /// multi-line field are not, as they need the text as laid out.
    pub fn edit(&mut self, keystroke: &Keystroke, paste: impl FnOnce() -> Option<String>) -> bool {
        let m = keystroke.modifiers;
        let ctrl = m.control && !m.platform && !m.alt;
        let reach = |word: bool, line: bool| match (line, word) {
            (true, _) => Reach::Line,
            (_, true) => Reach::Word,
            _ => Reach::Char,
        };
        let plain = !m.platform && !m.alt && !m.control;
        match keystroke.key.as_str() {
            // An arrow collapses a selection to its side.
            "left" | "right" if plain && !m.shift && !self.selection().is_empty() => {
                let range = self.selection();
                self.caret = if keystroke.key == "left" {
                    range.start
                } else {
                    range.end
                };
                self.anchor = None;
            }
            "left" => self.step(false, reach(m.alt, m.platform), m.shift),
            "right" => self.step(true, reach(m.alt, m.platform), m.shift),
            "up" | "down" if self.multiline => return false,
            "p" | "n" if ctrl && self.multiline => return false,
            // One line: up goes to its start, down to its end.
            "up" => self.go(0, m.shift),
            "down" => self.go(self.text.len(), m.shift),
            "p" if ctrl => self.go(0, m.shift),
            "n" if ctrl => self.go(self.text.len(), m.shift),
            "home" => self.step(false, Reach::Line, m.shift),
            "end" => self.step(true, Reach::Line, m.shift),
            "a" if ctrl => self.step(false, Reach::Line, m.shift),
            "e" if ctrl => self.step(true, Reach::Line, m.shift),
            "b" if ctrl => self.step(false, Reach::Char, m.shift),
            "f" if ctrl => self.step(true, Reach::Char, m.shift),
            "backspace" => self.delete(false, reach(m.alt, m.platform)),
            "delete" => self.delete(true, reach(m.alt, m.platform)),
            "h" if ctrl => self.delete(false, Reach::Char),
            "d" if ctrl => self.delete(true, Reach::Char),
            "k" if ctrl => self.kill(),
            "t" if ctrl => self.transpose(),
            "o" if ctrl && self.multiline => {
                self.replace("\n");
                self.caret -= 1;
            }
            "a" if m.platform => {
                self.anchor = Some(0);
                self.caret = self.text.len();
            }
            "enter" if self.multiline && !m.platform && !m.control => self.replace("\n"),
            "v" if m.platform => match paste() {
                Some(pasted) if self.multiline => self.replace(&pasted),
                Some(pasted) => self.replace(pasted.replace(['\n', '\r'], " ").trim()),
                None => return false,
            },
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

    /// Puts the caret at `to`, selecting from where it was with `select`.
    pub fn go(&mut self, to: usize, select: bool) {
        let to = self.boundary(to);
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

    /// Selects the word around `at`, for a double click.
    pub fn select_word(&mut self, at: usize) {
        let at = self.boundary(at);
        let start = self.text[..at]
            .char_indices()
            .rev()
            .take_while(|&(_, c)| is_word(c))
            .last()
            .map_or(at, |(i, _)| i);
        let end = self.text[at..]
            .char_indices()
            .find(|&(_, c)| !is_word(c))
            .map_or(self.text.len(), |(i, _)| at + i);
        self.anchor = (start < end).then_some(start);
        self.caret = end;
    }

    /// The nearest char boundary at or before `at`, within the text.
    fn boundary(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// Moves the caret, selecting on the way with `select`.
    fn step(&mut self, forward: bool, reach: Reach, select: bool) {
        let to = self.target(self.caret, forward, reach);
        self.go(to, select);
    }

    /// Deletes the selection, or from the caret as far as `reach`.
    fn delete(&mut self, forward: bool, reach: Reach) {
        if self.selection().is_empty() {
            let to = self.target(self.caret, forward, reach);
            self.anchor = Some(to);
        }
        self.replace("");
    }

    /// ⌃K: to the line's end, or the line break when already there.
    fn kill(&mut self) {
        let end = self.target(self.caret, true, Reach::Line);
        let to = match end == self.caret {
            true => self.target(self.caret, true, Reach::Char),
            false => end,
        };
        self.anchor = Some(to);
        self.replace("");
    }

    /// ⌃T: the characters around the caret change places, the last two at a
    /// line's end, and the caret goes past them.
    fn transpose(&mut self) {
        let start = self.target(self.caret, false, Reach::Line);
        let end = self.target(self.caret, true, Reach::Line);
        let at = match self.caret == end {
            true => self.target(self.caret, false, Reach::Char),
            false => self.caret,
        };
        let (from, to) = (
            self.target(at, false, Reach::Char),
            self.target(at, true, Reach::Char),
        );
        if at <= start || to == at {
            return;
        }
        let swapped = format!("{}{}", &self.text[at..to], &self.text[from..at]);
        self.anchor = Some(from);
        self.caret = to;
        self.replace(&swapped);
    }

    /// Where a move from `at` lands.
    fn target(&self, at: usize, forward: bool, reach: Reach) -> usize {
        let text = &self.text;
        match (reach, forward) {
            (Reach::Line, true) => text[at..].find('\n').map_or(text.len(), |i| at + i),
            (Reach::Line, false) => text[..at].rfind('\n').map_or(0, |i| i + 1),
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
            ..LineEdit::default()
        }
    }

    /// A key on `edit`, ⌘V pasting "XY".
    fn press(edit: &mut LineEdit, keys: &str) -> bool {
        let mut keystroke = Keystroke::parse(keys).unwrap();
        if keystroke.key.chars().count() == 1 && keystroke.modifiers.number_of_modifiers() == 0 {
            keystroke.key_char = Some(keystroke.key.clone());
        }
        edit.edit(&keystroke, || Some("X\nY".into()))
    }

    fn after(text: &str, caret: usize, keys: &str) -> (String, usize) {
        let mut edit = LineEdit::multiline(text);
        edit.caret = caret;
        press(&mut edit, keys);
        (edit.text, edit.caret)
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
        edit.step(false, Reach::Line, false);
        assert_eq!(edit.caret, 0);
        edit.step(true, Reach::Line, false);
        assert_eq!(edit.caret, 17);
    }

    #[test]
    fn shift_selects_and_an_arrow_collapses_it() {
        let mut edit = at("stable", 6);
        press(&mut edit, "shift-left");
        press(&mut edit, "shift-left");
        assert_eq!(edit.selection(), 4..6);
        press(&mut edit, "right");
        assert_eq!((edit.caret, edit.anchor), (6, None));
        // Selecting back to where it started selects nothing.
        press(&mut edit, "shift-left");
        press(&mut edit, "shift-right");
        assert_eq!(edit.anchor, None);
        press(&mut edit, "cmd-a");
        assert_eq!(edit.selection(), 0..6);
        press(&mut edit, "x");
        assert_eq!(edit.text, "x");
    }

    #[test]
    fn deleting_takes_a_char_a_word_a_line_or_the_selection() {
        let mut edit = at("*-lts", 5);
        edit.delete(false, Reach::Word);
        assert_eq!((edit.text.as_str(), edit.caret), ("*-", 2));
        edit.delete(false, Reach::Char);
        assert_eq!(edit.text, "*");
        let mut edit = at("v2410", 0);
        edit.delete(true, Reach::Char);
        assert_eq!((edit.text.as_str(), edit.caret), ("2410", 0));
        edit.delete(true, Reach::Line);
        assert_eq!(edit.text, "");
        assert_eq!(after("a\nbc", 4, "cmd-backspace"), ("a\n".into(), 2));
        assert_eq!(after("one two", 7, "alt-backspace"), ("one ".into(), 4));
    }

    #[test]
    fn lines_have_their_own_ends() {
        assert_eq!(after("a\nbc", 4, "cmd-left"), ("a\nbc".into(), 2));
        assert_eq!(after("a\nbc", 2, "cmd-right"), ("a\nbc".into(), 4));
        assert_eq!(after("ab\ncd", 4, "ctrl-a"), ("ab\ncd".into(), 3));
        assert_eq!(after("ab\ncd", 1, "ctrl-k"), ("a\ncd".into(), 1));
        assert_eq!(after("ab\ncd", 2, "ctrl-k"), ("abcd".into(), 2));
        assert_eq!(after("ab", 1, "enter"), ("a\nb".into(), 2));
        assert_eq!(after("ab", 1, "ctrl-o"), ("a\nb".into(), 1));
        assert_eq!(after("ab", 1, "cmd-v"), ("aX\nYb".into(), 4));
        // Up and down are for the view, which knows where lines wrap.
        assert!(!press(&mut LineEdit::multiline("a"), "up"));
    }

    #[test]
    fn one_line_keeps_to_one_line() {
        let mut edit = LineEdit::new("ab");
        edit.caret = 1;
        assert!(!press(&mut edit, "enter"));
        press(&mut edit, "cmd-v");
        assert_eq!(edit.text, "aX Yb");
        press(&mut edit, "up");
        assert_eq!(edit.caret, 0);
        press(&mut edit, "shift-down");
        assert_eq!(edit.selection(), 0..5);
    }

    #[test]
    fn ctrl_t_swaps_around_the_caret() {
        assert_eq!(after("abc", 1, "ctrl-t"), ("bac".into(), 2));
        assert_eq!(after("abc\nd", 3, "ctrl-t"), ("acb\nd".into(), 3));
        assert_eq!(after("a\nb", 2, "ctrl-t"), ("a\nb".into(), 2));
    }

    #[test]
    fn a_double_click_selects_a_word() {
        let mut edit = at("fix the_bar now", 0);
        edit.select_word(6);
        assert_eq!(edit.selection(), 4..11);
    }

    #[test]
    fn moves_keep_to_char_boundaries() {
        let mut edit = at("чу", 4);
        edit.step(false, Reach::Char, false);
        assert_eq!(edit.caret, 2);
        edit.delete(false, Reach::Char);
        assert_eq!((edit.text.as_str(), edit.caret), ("у", 0));
        edit.go(1, false);
        assert_eq!(edit.caret, 0);
    }
}
