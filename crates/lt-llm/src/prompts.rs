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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_byte_identical_to_the_frozen_template() {
        assert_eq!(HyMt2Prompts::user_message("我也想办一个，伟大的公司。"), "Translate the following text into English. Note that you should only output the translated result without any additional explanation:\n\n我也想办一个，伟大的公司。");
    }
}
