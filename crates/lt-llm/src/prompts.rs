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

    /// Separates the lines of a window. The model keeps it in place, so the
    /// English splits back into the same lines.
    pub const SEPARATOR: &'static str = "<br>";

    /// The model card's delimiter template, written as one paragraph that ends
    /// in a colon like `PREFIX`. In this form the model drops or merges
    /// separators less often than with the card's two-paragraph wording.
    pub const WINDOW_PREFIX: &'static str = "Please accurately translate the following text into English. You must retain the exact same number of delimiters in the translation. Strictly do not omit, escape, or translate these symbols, and pay close attention to their placement:\n\n";

    /// The earlier lines' sources and the new line, joined by `SEPARATOR`.
    pub fn window_source(earlier: &[(String, String)], text: &str) -> String {
        let mut source = String::new();
        for (line, _) in earlier {
            source.push_str(line);
            source.push_str(Self::SEPARATOR);
        }
        source.push_str(text);
        source
    }

    /// The earlier lines' English, each followed by `SEPARATOR`.
    pub fn window_prefill(earlier: &[(String, String)]) -> String {
        let mut prefill = String::new();
        for (_, english) in earlier {
            prefill.push_str(english);
            prefill.push_str(Self::SEPARATOR);
        }
        prefill
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

    pub fn window_message(source: &str) -> String {
        let mut prompt = String::with_capacity(Self::WINDOW_PREFIX.len() + source.len());
        prompt.push_str(Self::WINDOW_PREFIX);
        prompt.push_str(source);
        prompt
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

    #[test]
    fn window_prompt_is_byte_identical_to_the_frozen_template() {
        let earlier = [
            (
                "我们去公园，".to_owned(),
                "We're going to the park,".to_owned(),
            ),
            ("好的。".to_owned(), "Okay.".to_owned()),
        ];
        let source = HyMt2Prompts::window_source(&earlier, "然后吃饭。");
        assert_eq!(
            HyMt2Prompts::window_prefill(&earlier),
            "We're going to the park,<br>Okay.<br>"
        );
        assert_eq!(HyMt2Prompts::window_message(&source), "Please accurately translate the following text into English. You must retain the exact same number of delimiters in the translation. Strictly do not omit, escape, or translate these symbols, and pay close attention to their placement:\n\n我们去公园，<br>好的。<br>然后吃饭。");
    }
}
