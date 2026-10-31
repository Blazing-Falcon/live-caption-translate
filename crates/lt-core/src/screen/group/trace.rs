//! Records why each pair of lines was joined or kept apart, for `--explain`.

use super::engine::Node;
use crate::screen::line::{LayoutScript, Line, TextSizeBasis};
use serde::Serialize;
use std::{collections::HashMap, rc::Rc};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextLineDecision {
    pub kind: &'static str,
    pub previous: String,
    pub current: String,
    pub previous_script: LayoutScript,
    pub current_script: LayoutScript,
    pub vertical_gap: f64,
    pub left_delta: f64,
    pub center_delta: f64,
    pub right_delta: f64,
    pub alignment_delta: f64,
    pub text_size_ratio: f64,
    pub width_ratio: f64,
    pub line_advance: f64,
    pub leading_bar: f64,
    pub solid_bar: f64,
    pub joined: bool,
    pub rule: &'static str,
    pub previous_id: String,
    pub current_id: String,
    pub size_basis: TextSizeBasis,
    pub previous_size_value: f64,
    pub current_size_value: f64,
    pub advance_pixels: f64,
    pub advance_denominator: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TracedLine {
    pub id: String,
    pub block: Line,
    pub source_ids: Vec<String>,
}

#[derive(Default, Debug, Serialize)]
pub struct GroupingTrace {
    pub blocks: Vec<TracedLine>,
    pub lines: Vec<TracedLine>,
    pub groups: Vec<Vec<String>>,
    #[serde(skip)]
    block_ids: HashMap<usize, String>,
    #[serde(skip)]
    line_ids: HashMap<usize, String>,
    #[serde(skip)]
    pending: Vec<(Node, Vec<String>)>,
}

fn key(node: &Node) -> usize {
    Rc::as_ptr(node) as usize
}

impl GroupingTrace {
    pub(super) fn register_blocks(&mut self, blocks: &[Node]) {
        for block in blocks {
            let id = format!("b{}", self.blocks.len());
            self.block_ids.insert(key(block), id.clone());
            self.blocks.push(TracedLine {
                id,
                block: (**block).clone(),
                source_ids: Vec::new(),
            });
        }
    }
    pub(super) fn register_line(&mut self, line: &Node, sources: &[Node]) {
        let ids = sources.iter().map(|b| self.block_id(b)).collect();
        self.pending.push((Rc::clone(line), ids));
    }
    pub(super) fn order_lines(&mut self, order: &[Node]) {
        for line in order {
            let sources = self
                .pending
                .iter()
                .find(|(candidate, _)| Rc::ptr_eq(candidate, line))
                .map(|(_, sources)| sources.clone())
                .unwrap_or_default();
            let id = format!("L{}", self.lines.len());
            self.line_ids.insert(key(line), id.clone());
            self.lines.push(TracedLine {
                id,
                block: (**line).clone(),
                source_ids: sources,
            });
        }
    }
    pub(super) fn register_groups(&mut self, groups: &[Vec<Node>]) {
        for group in groups {
            self.groups
                .push(group.iter().map(|line| self.line_id(line)).collect());
        }
    }
    pub(super) fn block_id(&self, block: &Node) -> String {
        self.block_ids
            .get(&key(block))
            .cloned()
            .unwrap_or("?".into())
    }
    pub(super) fn line_id(&self, line: &Node) -> String {
        self.line_ids.get(&key(line)).cloned().unwrap_or("?".into())
    }
}
