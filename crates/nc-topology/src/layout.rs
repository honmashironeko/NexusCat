use crate::edge::EdgeCrossing;
use crate::mapping;
use crate::neighbors::{neighbors_for, Neighbors};
use nc_core::{
    Direction, DisplayNode, DisplayNodeId, NexusError, Point, Rect, Result, ScreenOrientation,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub const MAX_LAYOUT_ORIGIN_PX: f64 = 1_000_000.0;

pub const LAYOUT_GAP_PX: f64 = 24.0;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub nodes: Vec<DisplayNode>,
    pub revision: u64,
    #[serde(default)]
    pub active_node_ids: std::collections::HashSet<DisplayNodeId>,
    #[serde(default)]
    pub orientation_overrides: HashMap<DisplayNodeId, ScreenOrientation>,
    #[serde(default)]
    pub remote_desktop_origins: HashMap<DisplayNodeId, Point>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowStep {
    Move(DisplayNodeId),
    EdgeStop(Direction),
}

impl Layout {
    pub fn new(nodes: Vec<DisplayNode>) -> Self {
        let active_node_ids = nodes.iter().map(|node| node.id.clone()).collect();
        Self {
            nodes,
            revision: 0,
            active_node_ids,
            orientation_overrides: HashMap::new(),
            remote_desktop_origins: HashMap::new(),
        }
    }

    pub fn orientation_override(&self, id: &DisplayNodeId) -> Option<ScreenOrientation> {
        self.orientation_overrides.get(id).copied()
    }

    pub fn apply_orientation_overrides(&mut self) -> bool {
        let mut changed = false;
        let mut resized = HashSet::new();
        for node in &mut self.nodes {
            let Some(orientation) = self.orientation_overrides.get(&node.id).copied() else {
                continue;
            };
            let resolution = orientation.apply(node.resolution);
            let physical = orientation.apply_physical(node.physical);
            if node.resize_centered(resolution) {
                resized.insert(node.id.clone());
                changed = true;
            }
            if node.physical != physical {
                node.physical = physical;
                changed = true;
            }
        }
        if self.resolve_overlaps(&resized) {
            changed = true;
        }
        changed
    }

    pub fn apply_resolution_changes(&mut self, previous: &[DisplayNode]) -> bool {
        let previous: HashMap<&DisplayNodeId, &DisplayNode> =
            previous.iter().map(|node| (&node.id, node)).collect();
        let mut changed = false;
        let mut resized = HashSet::new();
        for node in &mut self.nodes {
            let Some(old) = previous.get(&node.id) else {
                continue;
            };
            if old.resolution == node.resolution {
                continue;
            }
            let center = old.center();
            node.origin = Point::new(
                center.x - node.resolution.width as f64 / 2.0,
                center.y - node.resolution.height as f64 / 2.0,
            );
            resized.insert(node.id.clone());
            changed = true;
        }
        if self.resolve_overlaps(&resized) {
            changed = true;
        }
        changed
    }

    pub fn resolve_overlaps(&mut self, ids: &HashSet<DisplayNodeId>) -> bool {
        let mut targets: Vec<DisplayNodeId> = ids
            .iter()
            .filter(|id| self.active_node_ids.contains(*id))
            .cloned()
            .collect();
        targets.sort();
        let mut changed = false;
        for id in targets {
            let Some(index) = self.nodes.iter().position(|node| node.id == id) else {
                continue;
            };
            if !self.node_overlaps_any(index) {
                continue;
            }
            let Some(origin) = self.nearest_free_origin(index) else {
                continue;
            };
            if self.nodes[index].origin != origin {
                self.nodes[index].origin = origin;
                changed = true;
            }
        }
        changed
    }

    fn node_overlaps_any(&self, index: usize) -> bool {
        let bounds = self.nodes[index].bounds();
        self.nodes.iter().enumerate().any(|(other, node)| {
            other != index
                && self.active_node_ids.contains(&node.id)
                && rects_overlap(bounds, node.bounds())
        })
    }

    fn nearest_free_origin(&self, index: usize) -> Option<Point> {
        let node = &self.nodes[index];
        let current = node.origin;
        let mut best: Option<(f64, Point)> = None;
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            let Some(candidate) = self.clear_origin_in_direction(index, direction) else {
                continue;
            };
            let distance = (candidate.x - current.x).abs() + (candidate.y - current.y).abs();
            if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, candidate));
            }
        }
        best.map(|(_, origin)| origin)
    }

    fn clear_origin_in_direction(&self, index: usize, direction: Direction) -> Option<Point> {
        let node = &self.nodes[index];
        let width = node.resolution.width as f64;
        let height = node.resolution.height as f64;
        let mut origin = node.origin;
        for _ in 0..=self.nodes.len() {
            let bounds = Rect::new(origin.x, origin.y, width, height);
            let overlapping: Vec<Rect> = self
                .nodes
                .iter()
                .enumerate()
                .filter(|(other, candidate)| {
                    *other != index && self.active_node_ids.contains(&candidate.id)
                })
                .map(|(_, candidate)| candidate.bounds())
                .filter(|candidate| rects_overlap(bounds, *candidate))
                .collect();
            if overlapping.is_empty() {
                return Some(origin);
            }
            match direction {
                Direction::Left => {
                    let left = overlapping
                        .iter()
                        .map(|bounds| bounds.x)
                        .fold(f64::MAX, f64::min);
                    origin.x = left - LAYOUT_GAP_PX - width;
                }
                Direction::Right => {
                    let right = overlapping
                        .iter()
                        .map(|bounds| bounds.right())
                        .fold(f64::MIN, f64::max);
                    origin.x = right + LAYOUT_GAP_PX;
                }
                Direction::Up => {
                    let top = overlapping
                        .iter()
                        .map(|bounds| bounds.y)
                        .fold(f64::MAX, f64::min);
                    origin.y = top - LAYOUT_GAP_PX - height;
                }
                Direction::Down => {
                    let bottom = overlapping
                        .iter()
                        .map(|bounds| bounds.bottom())
                        .fold(f64::MIN, f64::max);
                    origin.y = bottom + LAYOUT_GAP_PX;
                }
            }
        }
        None
    }

    pub fn node(&self, id: &DisplayNodeId) -> Option<&DisplayNode> {
        self.nodes.iter().find(|node| &node.id == id)
    }

    pub fn is_active(&self, id: &DisplayNodeId) -> bool {
        self.active_node_ids.contains(id)
    }

    pub fn local_node(&self) -> Option<&DisplayNode> {
        self.nodes
            .iter()
            .filter(|node| node.is_local && self.active_node_ids.contains(&node.id))
            .min_by_key(|node| (!node.is_primary, node.monitor_index))
    }

    pub fn neighbors(&self, id: &DisplayNodeId) -> Option<Neighbors> {
        if !self.is_active(id) {
            return None;
        }
        let active: Vec<DisplayNode> = self
            .nodes
            .iter()
            .filter(|node| self.active_node_ids.contains(&node.id))
            .cloned()
            .collect();
        let node = active.iter().find(|node| &node.id == id)?;
        Some(neighbors_for(node, &active))
    }

    pub fn neighbor(&self, id: &DisplayNodeId, direction: Direction) -> Option<&DisplayNode> {
        let neighbor_id = self.neighbor_id(id, direction)?;
        self.node(&neighbor_id)
    }

    pub fn neighbor_id(&self, id: &DisplayNodeId, direction: Direction) -> Option<DisplayNodeId> {
        self.neighbors(id)?.in_direction(direction).cloned()
    }

    pub fn neighbor_for_edge(
        &self,
        local_node: &DisplayNodeId,
        direction: Direction,
    ) -> Option<&DisplayNode> {
        self.neighbor(local_node, direction)
    }

    pub fn step(&self, from: &DisplayNodeId, direction: Direction) -> FlowStep {
        match self.neighbor_id(from, direction) {
            Some(to) => FlowStep::Move(to),
            None => FlowStep::EdgeStop(direction),
        }
    }

    pub fn map_point(
        &self,
        from: &DisplayNodeId,
        to: &DisplayNodeId,
        point: Point,
    ) -> Option<Point> {
        Some(mapping::map_point(self.node(from)?, self.node(to)?, point))
    }

    pub fn map_edge_crossing(&self, crossing: &EdgeCrossing) -> Option<Point> {
        Some(mapping::map_edge_crossing(
            self.node(&crossing.from)?,
            self.node(&crossing.to)?,
            crossing.direction,
            crossing.exit_point,
        ))
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen = HashSet::with_capacity(self.nodes.len());
        for node in &self.nodes {
            if !seen.insert(node.id.clone()) {
                return Err(NexusError::invalid(format!(
                    "显示节点 ID 重复：{}",
                    node.id
                )));
            }
            if node.id.device_id() != Some(node.device_id) {
                return Err(NexusError::invalid(format!(
                    "显示节点 {} 的 ID 不属于设备 {}",
                    node.id, node.device_id
                )));
            }
            if node.resolution.is_empty() {
                return Err(NexusError::invalid(format!(
                    "显示节点 {} 的分辨率不能为零",
                    node.id
                )));
            }
            if !node.origin.x.is_finite() || !node.origin.y.is_finite() {
                return Err(NexusError::invalid(format!(
                    "显示节点 {} 的原点不是有限数值",
                    node.id
                )));
            }
            if node.origin.x.abs() > MAX_LAYOUT_ORIGIN_PX
                || node.origin.y.abs() > MAX_LAYOUT_ORIGIN_PX
            {
                return Err(NexusError::invalid(format!(
                    "显示节点 {} 的原点超出允许范围 ±{MAX_LAYOUT_ORIGIN_PX}px",
                    node.id
                )));
            }
            if !node.dpi.is_finite() || node.dpi < 0.0 {
                return Err(NexusError::invalid(format!(
                    "显示节点 {} 的 DPI 必须为非负数",
                    node.id
                )));
            }
            if !node.physical.width_mm.is_finite()
                || !node.physical.height_mm.is_finite()
                || node.physical.width_mm < 0.0
                || node.physical.height_mm < 0.0
            {
                return Err(NexusError::invalid(format!(
                    "显示节点 {} 的物理尺寸非法",
                    node.id
                )));
            }
        }
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)?;
        let layout: Self = serde_json::from_str(&contents).map_err(|error| {
            NexusError::Fatal(format!("布局文件 {} 解析失败：{error}", path.display()))
        })?;
        layout.validate()?;
        Ok(layout)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = path.as_ref();
        let contents = serde_json::to_string_pretty(self)
            .map_err(|error| NexusError::Internal(format!("布局序列化失败：{error}")))?;
        fs::write(path, contents)?;
        Ok(())
    }
}

fn rects_overlap(first: Rect, second: Rect) -> bool {
    first.x < second.right()
        && second.x < first.right()
        && first.y < second.bottom()
        && second.y < first.bottom()
}

