use nc_core::{Direction, DisplayNode, DisplayNodeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Neighbors {
    pub left: Option<DisplayNodeId>,
    pub right: Option<DisplayNodeId>,
    pub up: Option<DisplayNodeId>,
    pub down: Option<DisplayNodeId>,
}

impl Neighbors {
    pub fn in_direction(&self, direction: Direction) -> Option<&DisplayNodeId> {
        match direction {
            Direction::Left => self.left.as_ref(),
            Direction::Right => self.right.as_ref(),
            Direction::Up => self.up.as_ref(),
            Direction::Down => self.down.as_ref(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (Direction, &DisplayNodeId)> {
        [
            (Direction::Left, self.left.as_ref()),
            (Direction::Right, self.right.as_ref()),
            (Direction::Up, self.up.as_ref()),
            (Direction::Down, self.down.as_ref()),
        ]
        .into_iter()
        .filter_map(|(direction, id)| id.map(|id| (direction, id)))
    }
}

pub(crate) fn neighbors_for(node: &DisplayNode, nodes: &[DisplayNode]) -> Neighbors {
    let mut candidates: [Option<DisplayNodeId>; 4] = [None, None, None, None];
    for (index, direction) in DIRECTION_ORDER.iter().enumerate() {
        let Some(candidate) = best_in_direction(node, nodes, *direction) else {
            continue;
        };
        let opposite = direction.opposite();
        let Some(back) = best_in_direction(candidate, nodes, opposite) else {
            continue;
        };
        if back.id == node.id {
            candidates[index] = Some(candidate.id.clone());
        }
    }

    Neighbors {
        left: candidates[0].clone(),
        right: candidates[1].clone(),
        up: candidates[2].clone(),
        down: candidates[3].clone(),
    }
}

const DIRECTION_ORDER: [Direction; 4] = [
    Direction::Left,
    Direction::Right,
    Direction::Up,
    Direction::Down,
];

fn best_in_direction<'a>(
    node: &DisplayNode,
    nodes: &'a [DisplayNode],
    direction: Direction,
) -> Option<&'a DisplayNode> {
    let source_center = node.center();
    let source_bounds = node.bounds();
    let mut best: Option<(&DisplayNode, (f64, f64, f64))> = None;

    for candidate in nodes {
        if candidate.id == node.id || !perpendicular_overlaps(node, candidate, direction) {
            continue;
        }
        let candidate_center = candidate.center();
        let candidate_bounds = candidate.bounds();
        let dx = candidate_center.x - source_center.x;
        let dy = candidate_center.y - source_center.y;
        if dx.abs() < f64::EPSILON && dy.abs() < f64::EPSILON {
            continue;
        }
        let horizontal = dx.abs() >= dy.abs();
        let (candidate_direction, deviation) = if horizontal {
            if dx > 0.0 {
                (Direction::Right, dy.abs() / dx.abs().max(1.0))
            } else {
                (Direction::Left, dy.abs() / dx.abs().max(1.0))
            }
        } else if dy > 0.0 {
            (Direction::Down, dx.abs() / dy.abs().max(1.0))
        } else {
            (Direction::Up, dx.abs() / dy.abs().max(1.0))
        };
        if candidate_direction != direction {
            continue;
        }
        let gap = match direction {
            Direction::Right => candidate_bounds.x - source_bounds.right(),
            Direction::Left => source_bounds.x - candidate_bounds.right(),
            Direction::Down => candidate_bounds.y - source_bounds.bottom(),
            Direction::Up => source_bounds.y - candidate_bounds.bottom(),
        };
        let distance = dx * dx + dy * dy;
        let score = (gap, deviation, distance);
        if best.is_none_or(|(_, current)| score < current) {
            best = Some((candidate, score));
        }
    }

    best.map(|(candidate, _)| candidate)
}

fn perpendicular_overlaps(
    source: &DisplayNode,
    candidate: &DisplayNode,
    direction: Direction,
) -> bool {
    let source_bounds = source.bounds();
    let candidate_bounds = candidate.bounds();
    match direction {
        Direction::Left | Direction::Right => interval_overlap(
            source_bounds.y,
            source_bounds.bottom(),
            candidate_bounds.y,
            candidate_bounds.bottom(),
        ),
        Direction::Up | Direction::Down => interval_overlap(
            source_bounds.x,
            source_bounds.right(),
            candidate_bounds.x,
            candidate_bounds.right(),
        ),
    }
}

fn interval_overlap(first_start: f64, first_end: f64, second_start: f64, second_end: f64) -> bool {
    first_end.min(second_end) - first_start.max(second_start) > f64::EPSILON
}

