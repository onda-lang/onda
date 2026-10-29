//! Text selection and clipboard policy, independent of the host's text input system.
use crate::{
    editing::{Editor, Field},
    events::EventField,
    model::RunUi,
    scene::Target,
    Input, Key,
};
use serde_json::{json, Value};
use std::ops::Range;

pub(crate) struct Draft {
    pub target: Field,
    pub text: String,
    pub original: String,
    pub cursor: usize,
    pub anchor: usize,
    pub invalid: bool,
    pub preedit: Option<(String, usize, usize)>,
    pub mark: Option<[usize; 2]>,
}
impl Draft {
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }
    pub fn insert(&mut self, text: &str) {
        self.preedit = None;
        self.mark = None;
        let selection = self.selection();
        self.text.replace_range(selection.clone(), text);
        self.cursor = selection.start + text.len();
        self.anchor = self.cursor;
        self.invalid = false;
    }
    pub fn display(&self) -> (String, usize, [usize; 2], Option<[usize; 2]>) {
        if let Some((text, anchor, cursor)) = &self.preedit {
            let range = self.selection();
            let mut display = self.text.clone();
            display.replace_range(range.clone(), text);
            (
                display,
                range.start + cursor,
                [range.start + anchor, range.start + cursor],
                Some([range.start, range.start + text.len()]),
            )
        } else {
            (
                self.text.clone(),
                self.cursor,
                [self.anchor, self.cursor],
                self.mark,
            )
        }
    }
    fn move_cursor(&mut self, cursor: usize, extend: bool) {
        self.cursor = cursor;
        if !extend {
            self.anchor = cursor;
        }
    }
    fn accepts(&self, text: &str) -> Option<String> {
        let text = if matches!(&self.target, Field::Event(EventField {scalar, ..}) if scalar == "json")
        {
            text.replace(['\r', '\n', '\t'], " ")
        } else {
            text.to_owned()
        };
        (!text.chars().any(char::is_control) && text.len() <= 65536).then_some(text)
    }
}

impl Editor {
    /// Text systems share the same UTF-16 selection, validation and composition policy.
    /// Returns whether the caller should redraw the draft.
    pub(crate) fn input(&mut self, input: Input) -> bool {
        match input {
            Input::Text { text } => self.insert_text(&text),
            Input::Preedit {
                text,
                anchor,
                cursor,
            } => self.preedit(text, anchor, cursor),
            Input::TextMark { start, end } => self.mark_text(start, end),
            Input::EditText {
                text,
                anchor,
                cursor,
            } => self.replace_text(&text, anchor, cursor),
            Input::SelectText { index, extend } => self.select_text_position(index, extend),
            _ => false,
        }
    }

    fn select_text_position(&mut self, index: usize, extend: bool) -> bool {
        let Some(d) = &mut self.draft else {
            return false;
        };
        if d.preedit.is_some() || d.mark.is_some() {
            return false;
        }
        d.move_cursor(byte_offset(&d.text, index), extend);
        true
    }
    fn mark_text(&mut self, start: usize, end: usize) -> bool {
        if let Some(d) = &mut self.draft {
            d.mark =
                (start != end).then(|| [byte_offset(&d.text, start), byte_offset(&d.text, end)]);
            true
        } else {
            false
        }
    }
    /// Composition stays separate from the draft until the OS commits it.
    fn preedit(&mut self, text: String, anchor: usize, cursor: usize) -> bool {
        let Some(d) = &mut self.draft else {
            return false;
        };
        let Some(text) = d.accepts(&text) else {
            return false;
        };
        if d.text.len() - d.selection().len() + text.len() > 65536 {
            return false;
        }
        d.preedit = if text.is_empty() {
            None
        } else {
            let anchor = byte_offset(&text, anchor);
            let cursor = byte_offset(&text, cursor);
            Some((text, anchor, cursor))
        };
        true
    }
    fn insert_text(&mut self, text: &str) -> bool {
        let Some(d) = &mut self.draft else {
            return false;
        };
        let Some(text) = d.accepts(text) else {
            return false;
        };
        if d.text.len() - d.selection().len() + text.len() > 65536 {
            return false;
        }
        d.insert(&text);
        true
    }
    /// Host text systems use UTF-16 offsets, including selection direction.
    fn replace_text(&mut self, text: &str, anchor: usize, cursor: usize) -> bool {
        let Some(d) = &mut self.draft else {
            return false;
        };
        let Some(text) = d.accepts(text) else {
            return false;
        };
        let anchor = byte_offset(&text, anchor);
        let cursor = byte_offset(&text, cursor);
        if d.text == text && d.anchor == anchor && d.cursor == cursor {
            return false;
        }
        d.text = text;
        d.preedit = None;
        d.anchor = anchor;
        d.cursor = cursor;
        d.invalid = false;
        true
    }
    pub(crate) fn text_key(&mut self, key: Key, shift: bool) -> Option<String> {
        let d = self.draft.as_mut()?;
        let mut copied = None;
        let selection = d.selection();
        let prev = d.text[..d.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
        let next = d.cursor + d.text[d.cursor..].chars().next().map_or(0, char::len_utf8);
        match key {
            Key::SelectAll => {
                d.anchor = 0;
                d.cursor = d.text.len();
            }
            Key::Copy | Key::Cut => {
                if !selection.is_empty() {
                    copied = Some(d.text[selection].to_owned());
                    if matches!(key, Key::Cut) {
                        d.insert("");
                    }
                }
            }
            Key::Backspace | Key::Delete if !selection.is_empty() => d.insert(""),
            Key::Backspace => {
                d.anchor = prev;
                d.insert("");
            }
            Key::Delete => {
                d.anchor = next;
                d.insert("");
            }
            Key::ArrowLeft => d.move_cursor(
                if !shift && !selection.is_empty() {
                    selection.start
                } else {
                    prev
                },
                shift,
            ),
            Key::ArrowRight => d.move_cursor(
                if !shift && !selection.is_empty() {
                    selection.end
                } else {
                    next
                },
                shift,
            ),
            Key::Home => d.move_cursor(0, shift),
            Key::End => d.move_cursor(d.text.len(), shift),
            _ => return None,
        }
        d.invalid = false;
        copied
    }
}

impl RunUi {
    /// A host may overlay its native editor on this logical-pixel field rectangle.
    pub fn text_edit(&mut self) -> Option<Value> {
        self.prepare_scene();
        let d = self.editor.draft.as_ref()?;
        let target = Target::Field(d.target.clone());
        let rect = self
            .scene
            .hits
            .iter()
            .rev()
            .find(|h| h.target == target && h.rect.0[2] > 0.0 && h.rect.0[3] > 0.0)?
            .rect
            .0;
        let label = self.scene.labels.iter().find(|l| l.caret.is_some())?;
        Some(json!({"text":d.text,
            "anchor":d.text[..d.anchor].encode_utf16().count(),
            "cursor":d.text[..d.cursor].encode_utf16().count(),
            "rect":rect,"centered":label.style.centered,"invalid":d.invalid,
            "fontSize":label.size,"monospace":label.style.mono,
            "resettable":self.field_numeric(&d.target) && self.field_default(&d.target).is_some_and(|v|!v.is_null())}))
    }
}

pub(crate) fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (index, c) in text.char_indices() {
        units += c.len_utf16();
        if units > utf16 {
            return index;
        }
    }
    text.len()
}
