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
        let text = if turns.is_empty() {
            Self::user_message(text)
        } else {
            text.to_owned()
        };
        turns.push(("user", text));
        turns
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
