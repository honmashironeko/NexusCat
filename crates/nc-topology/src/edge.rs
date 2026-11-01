use crate::layout::Layout;
use nc_core::{Direction, DisplayNode, DisplayNodeId, Point, Rect};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EdgeConfig {
    pub hysteresis_px: f64,
    pub dwell_ms: u64,
}

const DIRECTION_REARM_DISTANCE_PX: f64 = 32.0;

impl Default for EdgeConfig {
    fn default() -> Self {
        Self {
            hysteresis_px: 2.0,
            dwell_ms: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeCrossing {
    pub from: DisplayNodeId,
    pub to: DisplayNodeId,
    pub direction: Direction,
    pub exit_point: Point,
}

#[derive(Debug, Clone)]
struct PendingCrossing {
    direction: Direction,
    target: DisplayNodeId,
    since_ms: u64,
    exit_point: Point,
}

#[derive(Debug, Clone, Default)]
pub struct EdgeDetector {
    config: EdgeConfig,
    pending: Option<PendingCrossing>,
    blocked_direction: Option<Direction>,
}

impl EdgeDetector {
    pub fn new(config: EdgeConfig) -> Self {
        Self {
            config,
            pending: None,
            blocked_direction: None,
        }
    }

    pub fn config(&self) -> EdgeConfig {
        self.config
    }

    pub fn reset(&mut self) {
        self.pending = None;
        self.blocked_direction = None;
    }

    pub fn clear_pending(&mut self) {
        self.pending = None;
    }

    pub fn block(&mut self, direction: Direction) {
        self.pending = None;
        self.blocked_direction = Some(direction);
    }

    pub fn update(
        &mut self,
        layout: &Layout,
        current_node: &DisplayNodeId,
        point: Point,
        now_ms: u64,
    ) -> Option<EdgeCrossing> {
        let Some(node) = layout.node(current_node) else {
            self.reset();
            return None;
        };
        let hysteresis = self.effective_hysteresis();

        if self.blocked_direction.is_some_and(|direction| {
            edge_distance(node.bounds(), direction, point) > DIRECTION_REARM_DISTANCE_PX
        }) {
            self.blocked_direction = None;
        }

        let Some((direction, target)) = self.candidate(layout, node, point, hysteresis) else {
            self.pending = None;
            return None;
        };

        let should_fire = match &mut self.pending {
            Some(pending) if pending.direction == direction && pending.target == target => {
                if now_ms < pending.since_ms {
                    pending.since_ms = now_ms;
                    false
                } else {
                    now_ms.saturating_sub(pending.since_ms) >= self.config.dwell_ms
                }
            }
            _ => {
                self.pending = Some(PendingCrossing {
                    direction,
                    target,
                    since_ms: now_ms,
                    exit_point: point,
                });
                self.config.dwell_ms == 0
            }
        };

        if !should_fire {
            return None;
        }

        let pending = self.pending.take()?;
        self.blocked_direction = Some(pending.direction);
        Some(EdgeCrossing {
            from: current_node.clone(),
            to: pending.target,
            direction: pending.direction,
            exit_point: pending.exit_point,
        })
    }

    fn candidate(
        &self,
        layout: &Layout,
        node: &DisplayNode,
        point: Point,
        hysteresis: f64,
    ) -> Option<(Direction, DisplayNodeId)> {
        let bounds = node.bounds();
        let neighbors = layout.neighbors(&node.id)?;
        let mut best: Option<(f64, Direction, DisplayNodeId)> = None;

        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            if self.blocked_direction == Some(direction) {
                continue;
            }
            let distance = edge_distance(bounds, direction, point);
            if distance > hysteresis {
                continue;
            }
            let Some(neighbor) = neighbors.in_direction(direction) else {
                continue;
            };
            let is_better = best
                .as_ref()
                .is_none_or(|(best_distance, _, _)| distance < *best_distance);
            if is_better {
                best = Some((distance, direction, neighbor.clone()));
            }
        }

        best.map(|(_, direction, target)| (direction, target))
    }

    fn effective_hysteresis(&self) -> f64 {
        if self.config.hysteresis_px.is_finite() {
            self.config.hysteresis_px.max(0.0)
        } else {
            EdgeConfig::default().hysteresis_px
        }
    }
}

fn edge_distance(bounds: Rect, direction: Direction, point: Point) -> f64 {
    match direction {
        Direction::Left => (point.x - bounds.x).abs(),
        Direction::Right => (point.x - bounds.right()).abs(),
        Direction::Up => (point.y - bounds.y).abs(),
        Direction::Down => (point.y - bounds.bottom()).abs(),
    }
}
