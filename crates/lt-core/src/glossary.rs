//! The session glossary: the names and terms whose English must not drift.
//!
//! Speech recognition hears a name differently from line to line (卓总, 周总, 朱总, 翟总 are one
//! person), and a small translator renders each hearing differently again. Replacing every
//! spelling with one canonical form before translation is what makes the English name stable;
//! the entry's English then goes into the prompt for the lines that mention it, and is checked
//! against the answer.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, PoisonError, RwLock};

/// One person or term.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlossaryEntry {
    /// The canonical source spelling, as the translation input should carry it.
    pub zh: String,
    /// The English the translation must contain.
    pub en: String,
    /// A hint the translator needs to choose the right word, such as a person's gender or role
    /// ("name, male, her boss"). Empty when the name alone is enough.
    pub note: String,
    /// Other spellings recognition produces for the same thing.
    pub variants: Vec<String>,
    /// Only a confirmed entry is applied; a suggestion the user has not answered stays inert.
    pub confirmed: bool,
}

/// A confirmed entry's source spelling, with the entry it maps to.
#[derive(Clone, Debug)]
struct Variant {
    text: String,
    entry: usize,
}

/// A session's confirmed entries, indexed for longest-first replacement.
#[derive(Clone, Debug, Default)]
pub struct Glossary {
    entries: Vec<GlossaryEntry>,
    /// Longest first, so a longer name wins where two start at the same position.
    variants: Vec<Variant>,
}

impl Glossary {
    /// Build from configured entries. Entries without a source or an English spelling, and
    /// unconfirmed entries, are dropped.
    pub fn new(entries: &[GlossaryEntry]) -> Self {
        let mut kept = Vec::new();
        let mut variants = Vec::new();
        for entry in entries {
            let zh = entry.zh.trim();
            let en = entry.en.trim();
            if zh.is_empty() || en.is_empty() || !entry.confirmed {
                continue;
            }
            let index = kept.len();
            variants.push(Variant {
                text: zh.to_owned(),
                entry: index,
            });
            for variant in &entry.variants {
                let variant = variant.trim();
                if !variant.is_empty() && variant != zh {
                    variants.push(Variant {
                        text: variant.to_owned(),
                        entry: index,
                    });
                }
            }
            kept.push(GlossaryEntry {
                zh: zh.to_owned(),
                en: en.to_owned(),
                note: entry.note.trim().to_owned(),
                variants: entry
                    .variants
                    .iter()
                    .map(|variant| variant.trim().to_owned())
                    .filter(|variant| !variant.is_empty() && variant != zh)
                    .collect(),
                confirmed: true,
            });
        }
        variants.sort_by(|a, b| {
            b.text
                .chars()
                .count()
                .cmp(&a.text.chars().count())
                .then_with(|| a.text.cmp(&b.text))
        });
        Self {
            entries: kept,
            variants,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Replace every spelling of a confirmed entry with its canonical form.
    ///
    /// Exact matches only: a name heard almost right is a different string, and a fuzzy
    /// replacement would rewrite ordinary words. A match that begins a longer ordinary word is
    /// left alone (周总 inside 周总理).
    pub fn normalize(&self, text: &str) -> String {
        if self.variants.is_empty() || text.is_empty() {
            return text.to_owned();
        }
        let characters: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        while at < characters.len() {
            let mut matched: Option<&Variant> = None;
            for variant in &self.variants {
                let length = variant.text.chars().count();
                if at + length > characters.len() {
                    continue;
                }
                let same = characters[at..at + length]
                    .iter()
                    .copied()
                    .eq(variant.text.chars());
                if same && !extends_to_another_word(&characters, at + length) {
                    matched = Some(variant);
                    break;
                }
            }
            match matched {
                Some(variant) => {
                    out.push_str(&self.entries[variant.entry].zh);
                    at += variant.text.chars().count();
                }
                None => {
                    out.push(characters[at]);
                    at += 1;
                }
            }
        }
        out
    }

    /// The confirmed entries this text mentions, in configuration order, at most `limit`.
    pub fn matches(&self, text: &str, limit: usize) -> Vec<&GlossaryEntry> {
        if self.variants.is_empty() || text.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut found = vec![false; self.entries.len()];
        for variant in &self.variants {
            if found[variant.entry] {
                continue;
            }
            let mut from = 0;
            while let Some(offset) = text[from..].find(&variant.text) {
                let start = from + offset;
                let end = start + variant.text.len();
                let after: Vec<char> = text[end..].chars().take(1).collect();
                if !extends_to_another_word(&after, 0) {
                    found[variant.entry] = true;
                    break;
                }
                from = end;
            }
        }
        self.entries
            .iter()
            .zip(found)
            .filter_map(|(entry, found)| found.then_some(entry))
            .take(limit)
            .collect()
    }

    /// The glossary block for one line, as (source, English with its note). Only the entries the
    /// line mentions, so a line without names pays nothing for the list.
    pub fn terms(&self, text: &str, limit: usize) -> Vec<(String, String)> {
        self.matches(text, limit)
            .into_iter()
            .map(|entry| {
                let english = if entry.note.is_empty() {
                    entry.en.clone()
                } else {
                    format!("{} ({})", entry.en, entry.note)
                };
                (entry.zh.clone(), english)
            })
            .collect()
    }
}

/// The glossary a running pipeline reads. The user edits names during a show, so a new list
/// replaces this one in place instead of restarting the session.
#[derive(Clone, Debug, Default)]
pub struct SharedGlossary(Arc<RwLock<Glossary>>);

impl SharedGlossary {
    pub fn new(entries: &[GlossaryEntry]) -> Self {
        Self(Arc::new(RwLock::new(Glossary::new(entries))))
    }

    /// Apply a new list; the next clause and the next translation use it.
    pub fn replace(&self, entries: &[GlossaryEntry]) {
        let glossary = Glossary::new(entries);
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = glossary;
    }

    pub fn normalize(&self, text: &str) -> String {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .normalize(text)
    }

    pub fn terms(&self, text: &str, limit: usize) -> Vec<(String, String)> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .terms(text, limit)
    }
}

/// Characters that turn an honorific into an ordinary word when they follow it: 周总 + 理 is
/// 总理 (premier), 朱总 + 裁 is 总裁 (a company's president). Chinese has no word boundaries, so
/// replacement needs this guard.
const COMPOUND_TAILS: [char; 11] = [
    '理', '裁', '统', '书', '编', '监', '管', '局', '部', '长', '员',
];

fn extends_to_another_word(characters: &[char], at: usize) -> bool {
    characters
        .get(at)
        .is_some_and(|next| COMPOUND_TAILS.contains(next))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(zh: &str, en: &str, variants: &[&str]) -> GlossaryEntry {
        GlossaryEntry {
            zh: zh.into(),
            en: en.into(),
            note: String::new(),
            variants: variants.iter().map(|v| (*v).to_owned()).collect(),
            confirmed: true,
        }
    }

    #[test]
    fn every_hearing_normalises_to_the_canonical_spelling() {
        let glossary = Glossary::new(&[entry("卓总", "President Zhuo", &["周总", "朱总", "翟总"])]);
        for heard in ["周总不满意", "朱总不满意", "翟总不满意", "卓总不满意"] {
            assert_eq!(glossary.normalize(heard), "卓总不满意");
        }
        assert_eq!(glossary.normalize("今天天气不错"), "今天天气不错");
    }

    #[test]
    fn the_longer_spelling_wins() {
        let glossary = Glossary::new(&[
            entry("舒天蓝", "Shu Tianlan", &["舒厅兰"]),
            entry("舒", "Shu", &[]),
        ]);
        assert_eq!(glossary.normalize("舒厅兰来了"), "舒天蓝来了");
        assert_eq!(glossary.normalize("舒来了"), "舒来了");
    }

    #[test]
    fn a_name_that_begins_an_ordinary_word_is_left_alone() {
        let glossary = Glossary::new(&[entry("周总", "President Zhou", &[])]);
        assert_eq!(glossary.normalize("周总理来了"), "周总理来了");
        assert_eq!(glossary.normalize("周总来了"), "周总来了");
        assert!(glossary.matches("周总理来了", 8).is_empty());
        assert_eq!(glossary.matches("周总来了", 8).len(), 1);
    }

    #[test]
    fn an_unconfirmed_entry_is_never_applied() {
        let mut suggested = entry("卓总", "President Zhuo", &["周总"]);
        suggested.confirmed = false;
        let glossary = Glossary::new(&[suggested]);
        assert!(glossary.is_empty());
        assert_eq!(glossary.normalize("周总不满意"), "周总不满意");
    }

    #[test]
    fn terms_carry_the_note_and_respect_the_limit() {
        let mut zhuo = entry("卓总", "President Zhuo", &["周总"]);
        zhuo.note = "name, male, her boss".into();
        let glossary = Glossary::new(&[zhuo, entry("肖律", "Lawyer Xiao", &[])]);
        assert_eq!(
            glossary.terms("我要跟周总出差", 8),
            [(
                "卓总".to_owned(),
                "President Zhuo (name, male, her boss)".to_owned()
            )]
        );
        assert!(glossary.terms("我要跟周总出差", 0).is_empty());
        assert!(glossary.terms("今天天气不错", 8).is_empty());
    }
}
