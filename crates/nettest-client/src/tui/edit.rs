//! Single-line text editor for the settings tabs.
//!
//! Why: the form used to append to the existing value with the cursor stuck at the end, so
//! changing a host meant backspacing over it first. Entering edit mode now selects the whole
//! value (drawn reversed); the first typed character replaces it, like a GUI field. Arrow keys
//! drop the selection and give an ordinary cursor. Pasted text arrives as a burst of chars, so
//! the first replaces the selection and the rest append, which is the overwrite the user meant.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    chars: Vec<char>,
    /// Char index; `chars.len()` = after the last char.
    cursor: usize,
    select_all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    Editing,
    Commit,
    Cancel,
}

impl Editor {
    pub fn new(initial: &str) -> Self {
        let chars: Vec<char> = initial.chars().collect();
        Self {
            cursor: chars.len(),
            chars,
            select_all: true,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn selected(&self) -> bool {
        self.select_all
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn on_key(&mut self, k: KeyEvent) -> EditOutcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Enter => return EditOutcome::Commit,
            KeyCode::Esc => return EditOutcome::Cancel,
            KeyCode::Char('a') if ctrl => {
                self.select_all = true;
                self.cursor = self.chars.len();
            }
            KeyCode::Char('u') if ctrl => {
                self.chars.clear();
                self.cursor = 0;
                self.select_all = false;
            }
            KeyCode::Char(c) if !ctrl => {
                if self.select_all {
                    self.chars.clear();
                    self.cursor = 0;
                    self.select_all = false;
                }
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.select_all {
                    self.chars.clear();
                    self.cursor = 0;
                    self.select_all = false;
                } else if self.cursor > 0 {
                    self.cursor -= 1;
                    self.chars.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.select_all {
                    self.chars.clear();
                    self.cursor = 0;
                    self.select_all = false;
                } else if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                if self.select_all {
                    self.select_all = false;
                    self.cursor = 0;
                } else {
                    self.cursor = self.cursor.saturating_sub(1);
                }
            }
            KeyCode::Right => {
                if self.select_all {
                    self.select_all = false;
                    self.cursor = self.chars.len();
                } else {
                    self.cursor = (self.cursor + 1).min(self.chars.len());
                }
            }
            KeyCode::Home => {
                self.select_all = false;
                self.cursor = 0;
            }
            KeyCode::End => {
                self.select_all = false;
                self.cursor = self.chars.len();
            }
            _ => {}
        }
        EditOutcome::Editing
    }

    /// The value as drawn: fully reversed while selected, otherwise the cursor cell reversed
    /// (a thin bar when the cursor is past the end).
    pub fn spans(&self, base: Style) -> Vec<Span<'static>> {
        let rev = base.add_modifier(Modifier::REVERSED);
        if self.select_all {
            let text = if self.chars.is_empty() {
                " ".to_string()
            } else {
                self.text()
            };
            return vec![Span::styled(text, rev)];
        }
        let before: String = self.chars[..self.cursor].iter().collect();
        let mut out = vec![Span::styled(before, base)];
        if self.cursor < self.chars.len() {
            out.push(Span::styled(self.chars[self.cursor].to_string(), rev));
            let after: String = self.chars[self.cursor + 1..].iter().collect();
            out.push(Span::styled(after, base));
        } else {
            out.push(Span::styled("▏", base));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut e = Editor::new("10.0.0.1");
        assert!(e.selected());
        assert_eq!(e.on_key(key(KeyCode::Char('h'))), EditOutcome::Editing);
        assert_eq!(e.text(), "h");
        assert!(!e.selected());
        e.on_key(key(KeyCode::Char('q')));
        assert_eq!(e.text(), "hq");
        assert_eq!(e.on_key(key(KeyCode::Enter)), EditOutcome::Commit);
    }

    #[test]
    fn backspace_and_delete_clear_the_selection() {
        let mut e = Editor::new("abc");
        e.on_key(key(KeyCode::Backspace));
        assert_eq!(e.text(), "");
        let mut e = Editor::new("abc");
        e.on_key(key(KeyCode::Delete));
        assert_eq!(e.text(), "");
        assert_eq!(e.on_key(key(KeyCode::Esc)), EditOutcome::Cancel);
    }

    #[test]
    fn arrows_drop_the_selection_and_move() {
        let mut e = Editor::new("abc");
        e.on_key(key(KeyCode::Left));
        assert!(!e.selected());
        assert_eq!(e.cursor(), 0);
        e.on_key(key(KeyCode::Right));
        e.on_key(key(KeyCode::Char('X')));
        assert_eq!(e.text(), "aXbc");
        e.on_key(key(KeyCode::End));
        e.on_key(key(KeyCode::Backspace));
        assert_eq!(e.text(), "aXb");
        e.on_key(key(KeyCode::Home));
        e.on_key(key(KeyCode::Delete));
        assert_eq!(e.text(), "Xb");

        let mut e = Editor::new("abc");
        e.on_key(key(KeyCode::Right));
        assert_eq!(e.cursor(), 3);
        e.on_key(key(KeyCode::Char('d')));
        assert_eq!(e.text(), "abcd");
    }

    #[test]
    fn ctrl_a_reselects_and_ctrl_u_clears() {
        let mut e = Editor::new("abc");
        e.on_key(key(KeyCode::Left));
        e.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(e.selected());
        e.on_key(key(KeyCode::Char('z')));
        assert_eq!(e.text(), "z");
        e.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "");
    }

    #[test]
    fn spans_mark_selection_then_cursor() {
        let e = Editor::new("abc");
        let s = e.spans(Style::default());
        assert_eq!(s.len(), 1);
        assert!(s[0].style.add_modifier.contains(Modifier::REVERSED));
        let mut e = Editor::new("abc");
        e.on_key(key(KeyCode::Left));
        let s = e.spans(Style::default());
        assert_eq!(s[0].content, "");
        assert_eq!(s[1].content, "a");
        assert_eq!(s[2].content, "bc");
        e.on_key(key(KeyCode::End));
        let s = e.spans(Style::default());
        assert_eq!(s[0].content, "abc");
        assert_eq!(s[1].content, "▏");
    }
}
