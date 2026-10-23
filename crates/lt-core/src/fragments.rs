//! Whether a closed clause is really finished, or the speaker is still going.
//!
//! A pause is not a sentence end. When the recognizer cuts at a pause and the words say the
//! thought continues, joining the next clause back on is better than two captions. The cues come
//! from the words, because a time or length test cannot tell a short complete line (`谢谢`) from
//! an unfinished one (`那就当做`).

/// A clause cannot end on these: function words and prepositions that need a complement.
/// `样` and `过` are left out: 怎么样 and 我没去过 end a sentence.
const CONTINUATION_TAIL: &str = "的地得把被跟和与对在从向往给让使为比由就是会能要想该敢肯也都还又很太更最不没别挺真才这个些位件种但而并且着说做当成叫";

/// A pronoun at the end is either an object, which completes the clause (谢谢你, 相信我), or a
/// subject still waiting for its verb (然后他, 跟我).
const PRONOUNS: &str = "我你他她它咱您";

/// Two-character endings that continue in the same way.
const CONTINUATION_TAIL_PAIRS: [&str; 5] = ["我们", "你们", "他们", "咱们", "的话"];

/// The next clause starting with one of these is the same thought continuing. Matched as
/// prefixes: `还有` is a connective, `有没有` is not.
///
/// `那` and `这` are left out: both false hits in the measurement started with `这`, and no real
/// continuation needed either.
const CONTINUATION_HEAD: [&str; 22] = [
    "而且", "如果", "但是", "然后", "所以", "因为", "不过", "另外", "还有", "只是", "就是", "还是",
    "虽然", "可是", "但", "而", "并", "又", "也", "就", "还", "可",
];

/// Marks that end a sentence outright. A clause that has one is finished whatever else is true.
const SENTENCE_MARKS: &str = "。？！?!";

/// Particles that end a sentence, so a trailing one means finished unless the next clause's own
/// first word says otherwise.
const FINAL_PARTICLES: &str = "了吧啊嘛呢哦呀";

/// Whether the clause ending at `text` is still going, given the first word of what follows.
///
/// `next_head` is the opening of the next clause when it has already been heard; `None` when it
/// has not, in which case only the tail can decide.
pub fn looks_unfinished(text: &str, next_head: Option<&str>) -> bool {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return false;
    }
    let head_cue = next_head.is_some_and(starts_with_connective);
    let last = match trimmed.chars().next_back() {
        Some(last) => last,
        None => return false,
    };
    if SENTENCE_MARKS.contains(last) {
        return false;
    }
    if FINAL_PARTICLES.contains(last) {
        // A sentence-final particle usually closes the thought; only the next clause's own
        // first word can overrule it.
        return head_cue;
    }
    if head_cue {
        return true;
    }
    tail_cue(trimmed)
}

/// The last word cannot end a sentence: a function word, a subject pronoun, or one of the
/// pairs.
fn tail_cue(text: &str) -> bool {
    // The punctuation that ended the cut is not part of the last word.
    let words = text.trim_end_matches(|character: char| !character.is_alphanumeric());
    // 是不是, 要不要, 有没有: a question that ends here, though its verb alone would not.
    if ends_with_a_not_a(words) {
        return false;
    }
    if CONTINUATION_TAIL_PAIRS
        .iter()
        .any(|pair| words.ends_with(pair))
    {
        return true;
    }
    let Some(last) = words.chars().next_back() else {
        return false;
    };
    if PRONOUNS.contains(last) {
        // A pronoun after a function word or a connective, or on its own, has no verb yet.
        let before = &words[..words.len() - last.len_utf8()];
        return before
            .chars()
            .next_back()
            .is_none_or(|character| CONTINUATION_TAIL.contains(character))
            || CONTINUATION_HEAD
                .iter()
                .any(|connective| before.ends_with(connective));
    }
    CONTINUATION_TAIL.contains(last)
}

fn ends_with_a_not_a(words: &str) -> bool {
    let last: Vec<char> = words.chars().rev().take(3).collect();
    matches!(last.as_slice(), [verb, '不' | '没', again] if verb == again)
}

fn starts_with_connective(text: &str) -> bool {
    let trimmed = text.trim_start();
    CONTINUATION_HEAD
        .iter()
        .any(|connective| trimmed.starts_with(connective))
}

/// How many Han characters the text holds, which bounds how much a held clause may grow.
pub fn han_count(text: &str) -> usize {
    text.chars()
        .filter(|character| ('\u{4e00}'..='\u{9fff}').contains(character))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clause_ending_on_a_function_word_is_unfinished() {
        assert!(looks_unfinished("那就当做", None));
        assert!(looks_unfinished("我希望我们", None));
        assert!(looks_unfinished("还是说", None));
        assert!(looks_unfinished("然后他", None));
        assert!(looks_unfinished("跟我，", None));
        assert!(looks_unfinished("他", None));
        assert!(looks_unfinished(
            "我大可不必跟贵公司合作，",
            Some("而且我十分怀疑")
        ));
    }

    #[test]
    fn a_sentence_mark_finishes_a_clause() {
        assert!(!looks_unfinished("那就当做。", None));
        assert!(!looks_unfinished("知道了！", None));
        assert!(!looks_unfinished("谢谢。", Some("而且我还有事")));
    }

    #[test]
    fn a_final_particle_finishes_unless_the_next_clause_continues() {
        assert!(!looks_unfinished("知道了", None));
        assert!(!looks_unfinished("好吧", None));
        assert!(looks_unfinished("好吧", Some("而且我还有事")));
    }

    #[test]
    fn a_connective_at_the_start_of_the_next_clause_holds_a_short_line() {
        // 谢谢 would otherwise read as complete; 而且 says the speaker is still going.
        assert!(looks_unfinished("谢谢", Some("而且我还有事")));
        assert!(looks_unfinished("好", Some("还有一件事")));
    }

    #[test]
    fn a_complete_short_line_is_not_held() {
        assert!(!looks_unfinished("谢谢", None));
        assert!(!looks_unfinished("不用了", None));
        assert!(!looks_unfinished("给我吧", None));
        // A pronoun that is the object, and a question that asks "or not", end here.
        for complete in ["谢谢你", "相信我，", "是不是", "要不要", "对不对", "有没有"]
        {
            assert!(!looks_unfinished(complete, None), "{complete}");
        }
        assert!(!looks_unfinished("怎么样", None));
        assert!(!looks_unfinished("我没去过", None));
        // A question that merely starts with 那 is not a continuation.
        assert!(!looks_unfinished("谢谢", Some("那是什么")));
    }

    #[test]
    fn a_connective_must_be_a_prefix_not_a_substring() {
        assert!(!looks_unfinished("谢谢", Some("有没有人说过")));
        assert!(looks_unfinished("谢谢", Some("还有一件事")));
    }
}
