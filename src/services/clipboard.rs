//! Clipboard history for the launcher (`;` prefix). Kept in memory only:
//! copied text often holds secrets, so nothing is written to disk, and
//! anything a password manager marks as private never gets here (see
//! `platform::clipboard`).

use std::{collections::VecDeque, time::SystemTime};

/// Longer copies (logs, whole files) are not worth keeping around.
pub const MAX_CHARS: usize = 100_000;

#[derive(Debug, Clone, PartialEq)]
pub struct ClipItem {
    pub text: String,
    pub at: SystemTime,
}

impl ClipItem {
    /// First non-empty line, whitespace collapsed (for a one-line row).
    pub fn preview(&self, max: usize) -> String {
        let line = self.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.chars().count() > max {
            collapsed.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
        } else {
            collapsed
        }
    }

    pub fn lines(&self) -> usize {
        self.text.lines().count().max(1)
    }
}

#[derive(Debug)]
pub struct ClipHistory {
    items: VecDeque<ClipItem>,
    max: usize,
}

impl ClipHistory {
    pub fn new(max: usize) -> Self {
        Self { items: VecDeque::new(), max: max.max(1) }
    }

    /// Newest first. Returns false when nothing changed.
    pub fn push(&mut self, text: String, at: SystemTime) -> bool {
        if text.trim().is_empty() || text.chars().count() > MAX_CHARS {
            return false;
        }
        if self.items.front().is_some_and(|i| i.text == text) {
            return false;
        }
        // Copying something again moves it to the top instead of duplicating it.
        if let Some(pos) = self.items.iter().position(|i| i.text == text) {
            self.items.remove(pos);
        }
        self.items.push_front(ClipItem { text, at });
        self.items.truncate(self.max);
        true
    }

    pub fn set_max(&mut self, max: usize) {
        self.max = max.max(1);
        self.items.truncate(self.max);
    }

    pub fn remove(&mut self, index: usize) -> Option<ClipItem> {
        self.items.remove(index)
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn get(&self, index: usize) -> Option<&ClipItem> {
        self.items.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ClipItem> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(s: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(s)
    }

    fn texts(h: &ClipHistory) -> Vec<&str> {
        h.iter().map(|i| i.text.as_str()).collect()
    }

    #[test]
    fn newest_first_deduplicated_and_bounded() {
        let mut h = ClipHistory::new(3);
        assert!(h.push("a".into(), t(1)));
        assert!(h.push("b".into(), t(2)));
        assert!(!h.push("b".into(), t(3)), "same as the newest");
        assert!(h.push("a".into(), t(4)), "older copy moves to the top");
        assert_eq!(texts(&h), ["a", "b"]);
        assert_eq!(h.get(0).unwrap().at, t(4));
        h.push("c".into(), t(5));
        h.push("d".into(), t(6));
        assert_eq!(texts(&h), ["d", "c", "a"]);
        h.set_max(2);
        assert_eq!(texts(&h), ["d", "c"]);
    }

    #[test]
    fn ignores_blank_and_huge_text() {
        let mut h = ClipHistory::new(5);
        assert!(!h.push("  \n\t".into(), t(1)));
        assert!(!h.push("x".repeat(MAX_CHARS + 1), t(1)));
        assert!(h.is_empty());
    }

    #[test]
    fn remove_and_clear() {
        let mut h = ClipHistory::new(5);
        h.push("a".into(), t(1));
        h.push("b".into(), t(2));
        assert_eq!(h.remove(1).map(|i| i.text), Some("a".into()));
        assert_eq!(h.remove(7), None);
        h.clear();
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn previews_first_line() {
        let item = ClipItem { text: "\n   fn main() {\n    println!();\n}".into(), at: t(0) };
        assert_eq!(item.preview(40), "fn main() {");
        assert_eq!(item.preview(5), "fn m…");
        assert_eq!(item.lines(), 4);
        let spaced = ClipItem { text: "a    b\tc".into(), at: t(0) };
        assert_eq!(spaced.preview(40), "a b c");
    }
}
