//! Picks how a block's English is laid out over its source lines.

use super::line::Line;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutIntent {
    #[default]
    Single,
    GroupReflow,
    AcrossInColumns,
}

pub fn sets_down(target: Option<&str>) -> bool {
    matches!(
        target.map(|t| t.trim().to_uppercase()).as_deref(),
        Some("JA" | "ZH-HANT" | "ZH" | "ZH-HANS" | "KO")
    )
}

pub fn intent_for_direction(block: &Line, direction: super::Direction) -> LayoutIntent {
    if direction == super::Direction::Vertical && !block.runs_across && !sets_down(Some("EN")) {
        LayoutIntent::AcrossInColumns
    } else {
        intent_for(block)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{geom::RectF, Direction};

    #[test]
    fn downward_targets_and_other_targets_keep_distinct_directions() {
        for target in ["JA", "ZH-HANT", "ZH", "ZH-HANS", "KO", "zh-hant"] {
            assert!(sets_down(Some(target)));
        }
        for target in [
            Some("EN"),
            Some("ID"),
            Some("DE"),
            Some("RU"),
            Some("EL"),
            Some("FI"),
            Some("XX"),
            Some(""),
            None,
        ] {
            assert!(!sets_down(target));
        }
    }

    #[test]
    fn columns_set_across_while_a_caption_keeps_its_horizontal_intent() {
        let column = Line {
            source_line_bounds: Some(vec![
                RectF::new(80.0, 10.0, 20.0, 20.0),
                RectF::new(80.0, 30.0, 20.0, 20.0),
            ]),
            ..Line::new("縦書き", RectF::new(80.0, 10.0, 20.0, 40.0))
        };
        let row = Line {
            runs_across: true,
            ..Line::new("名前", RectF::new(10.0, 100.0, 120.0, 20.0))
        };
        assert_eq!(
            intent_for_direction(&column, Direction::Vertical),
            LayoutIntent::AcrossInColumns
        );
        assert_eq!(
            intent_for_direction(&row, Direction::Vertical),
            LayoutIntent::Single
        );
    }
}
