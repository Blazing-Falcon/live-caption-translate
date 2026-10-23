//! The stable user-only Hy-MT2 prompt prefix is shared by warm-up and translation.
#[derive(Clone, Copy, Debug, Default)]
pub struct HyMt2Prompts;

impl HyMt2Prompts {
    pub const PREFIX: &'static str = "Translate the following text into English. Note that you should only output the translated result without any additional explanation:\n\n";

    pub fn user_message(text: &str) -> String {
        let mut prompt = String::with_capacity(Self::PREFIX.len() + text.len());
        prompt.push_str(Self::PREFIX);
        prompt.push_str(text);
        prompt
    }

    /// The earlier lines as finished chat turns, then the new line. Only the first user turn
    /// carries the instruction: the small model reads prompts slowly, and the turns after it
    /// keep the task without it.
    pub fn turns(earlier: &[(String, String)], text: &str) -> Vec<(&'static str, String)> {
        Self::turns_with_terms(earlier, &[], text)
    }

    /// As `turns`, with the glossary entries this line mentions in the last user turn.
    ///
    /// The block goes in the last turn, so the instruction and every earlier turn stay
    /// byte-identical and the server's cached prefix still covers them. With terms, the last
    /// turn repeats the whole documented shape: the terminology block, the instruction, then
    /// the line. Only the entries the line mentions, because a list on every line costs
    /// latency on a 1.8B model, and a block without its instruction makes the model answer
    /// instead of translate.
    pub fn turns_with_terms(
        earlier: &[(String, String)],
        terms: &[(String, String)],
        text: &str,
    ) -> Vec<(&'static str, String)> {
        let mut turns = Vec::with_capacity(earlier.len() * 2 + 1);
        for (line, english) in earlier {
            let line = if turns.is_empty() {
                Self::user_message(line)
            } else {
                line.clone()
            };
            turns.push(("user", line));
            turns.push(("assistant", english.clone()));
        }
        let text = if !terms.is_empty() {
            format!("{}{}", Self::term_block(terms), Self::user_message(text))
        } else if turns.is_empty() {
            Self::user_message(text)
        } else {
            text.to_owned()
        };
        turns.push(("user", text));
        turns
    }

    /// Hy-MT2's own terminology wording, one entry per line, with the source quoted as the
    /// model's own template quotes it.
    ///
    /// A term's note is carried in brackets after its English, but it is a hint for the
    /// translator, not part of the name: left inline the model copies it into the caption
    /// ("Lawyer Xiao (name, male)"). The notes therefore go on their own lines after the list.
    pub fn term_block(terms: &[(String, String)]) -> String {
        let mut block = String::from("Reference the following translations:\n");
        let mut notes = String::new();
        for (source, english) in terms {
            let (name, note) = english
                .split_once(" (")
                .map_or((english.as_str(), None), |(name, rest)| {
                    (name, rest.strip_suffix(')'))
                });
            block.push('`');
            block.push_str(source);
            block.push_str("` translates to `");
            block.push_str(name);
            block.push_str("`\n");
            if let Some(note) = note {
                notes.push_str(source);
                notes.push_str(" is ");
                notes.push_str(note);
                notes.push_str(". ");
            }
        }
        block.push('\n');
        if !notes.is_empty() {
            block.push_str("Context: ");
            block.push_str(notes.trim_end());
            block.push('\n');
            block.push('\n');
        }
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_byte_identical_to_the_frozen_template() {
        assert_eq!(HyMt2Prompts::user_message("我也想办一个，伟大的公司。"), "Translate the following text into English. Note that you should only output the translated result without any additional explanation:\n\n我也想办一个，伟大的公司。");
    }

    #[test]
    fn terms_go_before_the_instruction_and_their_notes_beside_it() {
        let terms = [
            (
                "卓总".to_owned(),
                "President Zhuo (name, male, her boss)".to_owned(),
            ),
            ("肖律".to_owned(), "Lawyer Xiao".to_owned()),
        ];
        assert_eq!(
            HyMt2Prompts::turns_with_terms(&[], &terms, "我要跟卓总出差几天，"),
            [(
                "user",
                format!(
                    "Reference the following translations:\n`卓总` translates to `President Zhuo`\n`肖律` translates to `Lawyer Xiao`\n\nContext: 卓总 is name, male, her boss.\n\n{}",
                    HyMt2Prompts::user_message("我要跟卓总出差几天，")
                )
            )]
        );
    }

    #[test]
    fn a_term_note_never_reaches_the_caption_as_part_of_the_name() {
        let block = HyMt2Prompts::term_block(&[(
            "卓总".to_owned(),
            "President Zhuo (name, male)".to_owned(),
        )]);
        assert!(!block.contains("`President Zhuo (name, male)`"));
        assert!(block.contains("`President Zhuo`"));
    }

    #[test]
    fn chat_turns_carry_the_instruction_only_in_the_first_turn() {
        let earlier = [
            (
                "我们去公园，".to_owned(),
                "We're going to the park,".to_owned(),
            ),
            ("好的。".to_owned(), "Okay.".to_owned()),
        ];
        assert_eq!(
            HyMt2Prompts::turns(&earlier, "然后吃饭。"),
            [
                ("user", "Translate the following text into English. Note that you should only output the translated result without any additional explanation:\n\n我们去公园，".to_owned()),
                ("assistant", "We're going to the park,".to_owned()),
                ("user", "好的。".to_owned()),
                ("assistant", "Okay.".to_owned()),
                ("user", "然后吃饭。".to_owned()),
            ]
        );
    }
}
