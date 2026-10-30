use super::line::Line;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutIntent {
    #[default]
    Single,
    GroupReflow,
}

pub fn intent_for(block: &Line) -> LayoutIntent {
    if block
        .source_line_bounds
        .as_ref()
        .is_some_and(|lines| lines.len() > 1)
    {
        LayoutIntent::GroupReflow
    } else {
        LayoutIntent::Single
    }
}
