import type { DisplayNode } from '../api/types';

export type LayoutDirection = 'left' | 'right' | 'up' | 'down';
export const LAYOUT_DIRECTIONS: LayoutDirection[] = ['left', 'right', 'up', 'down'];

const OPPOSITE_DIRECTION: Record<LayoutDirection, LayoutDirection> = {
  left: 'right',
  right: 'left',
  up: 'down',
  down: 'up',
};

export type NeighborMap = Record<LayoutDirection, DisplayNode | null>;

interface CenterPoint {
  x: number;
  y: number;
}

export interface RelativeDirection {
  direction: LayoutDirection;
  deviation: number;
  distance: number;
}

export function nodeCenter(node: DisplayNode): CenterPoint {
  return {
    x: node.origin.x + node.resolution.width / 2,
    y: node.origin.y + node.resolution.height / 2,
  };
}

export function relativeDirection(
  source: DisplayNode,
  candidate: DisplayNode,
): RelativeDirection | null {
  const sourceCenter = nodeCenter(source);
  const candidateCenter = nodeCenter(candidate);
  const dx = candidateCenter.x - sourceCenter.x;
  const dy = candidateCenter.y - sourceCenter.y;
  if (Math.abs(dx) < Number.EPSILON && Math.abs(dy) < Number.EPSILON) {
    return null;
  }
  const horizontal = Math.abs(dx) >= Math.abs(dy);
  const direction: LayoutDirection = horizontal
    ? dx > 0
      ? 'right'
      : 'left'
    : dy > 0
      ? 'down'
      : 'up';
  const deviation = horizontal
    ? Math.abs(dy) / Math.max(1, Math.abs(dx))
    : Math.abs(dx) / Math.max(1, Math.abs(dy));
  return {
    direction,
    deviation,
    distance: dx * dx + dy * dy,
  };
}

export function directionFromPrimary(
  nodes: DisplayNode[],
  nodeId: string,
): LayoutDirection | null {
  const local =
    nodes.find((node) => node.is_local && node.is_primary) ??
    nodes.find((node) => node.is_local);
  const node = nodes.find((candidate) => candidate.id === nodeId);
  if (!local || !node || local.id === node.id) {
    return null;
  }
  return relativeDirection(local, node)?.direction ?? null;
}

export function directionFromLocal(
  nodes: DisplayNode[],
  nodeId: string,
): LayoutDirection | null {
  const local =
    nodes.find((node) => node.is_local && node.is_primary) ??
    nodes.find((node) => node.is_local);
  const node = nodes.find((candidate) => candidate.id === nodeId);
  if (!local || !node || node.is_local) {
    return null;
  }
  return relativeDirection(local, node)?.direction ?? null;
}

interface Bounds {
  x: number;
  y: number;
  right: number;
  bottom: number;
}

function nodeBounds(node: DisplayNode): Bounds {
  return {
    x: node.origin.x,
    y: node.origin.y,
    right: node.origin.x + node.resolution.width,
    bottom: node.origin.y + node.resolution.height,
  };
}

function intervalOverlap(
  firstStart: number,
  firstEnd: number,
  secondStart: number,
  secondEnd: number,
): boolean {
  return Math.min(firstEnd, secondEnd) - Math.max(firstStart, secondStart) > 0;
}

function perpendicularOverlaps(
  source: DisplayNode,
  candidate: DisplayNode,
  direction: LayoutDirection,
): boolean {
  const sourceBounds = nodeBounds(source);
  const candidateBounds = nodeBounds(candidate);
  if (direction === 'left' || direction === 'right') {
    return intervalOverlap(
      sourceBounds.y,
      sourceBounds.bottom,
      candidateBounds.y,
      candidateBounds.bottom,
    );
  }
  return intervalOverlap(
    sourceBounds.x,
    sourceBounds.right,
    candidateBounds.x,
    candidateBounds.right,
  );
}

function edgeGap(
  source: DisplayNode,
  candidate: DisplayNode,
  direction: LayoutDirection,
): number {
  const sourceBounds = nodeBounds(source);
  const candidateBounds = nodeBounds(candidate);
  switch (direction) {
    case 'right':
      return candidateBounds.x - sourceBounds.right;
    case 'left':
      return sourceBounds.x - candidateBounds.right;
    case 'down':
      return candidateBounds.y - sourceBounds.bottom;
    case 'up':
      return sourceBounds.y - candidateBounds.bottom;
  }
}

function bestInDirection(
  source: DisplayNode,
  nodes: DisplayNode[],
  direction: LayoutDirection,
): DisplayNode | null {
  const sourceCenter = nodeCenter(source);
  let best: { node: DisplayNode; score: [number, number, number] } | null = null;
  for (const candidate of nodes) {
    if (
      candidate.id === source.id ||
      !perpendicularOverlaps(source, candidate, direction)
    ) {
      continue;
    }
    const candidateCenter = nodeCenter(candidate);
    const dx = candidateCenter.x - sourceCenter.x;
    const dy = candidateCenter.y - sourceCenter.y;
    if (Math.abs(dx) < Number.EPSILON && Math.abs(dy) < Number.EPSILON) {
      continue;
    }
    const horizontal = Math.abs(dx) >= Math.abs(dy);
    let candidateDirection: LayoutDirection;
    let deviation: number;
    if (horizontal) {
      candidateDirection = dx > 0 ? 'right' : 'left';
      deviation = Math.abs(dy) / Math.max(1, Math.abs(dx));
    } else {
      candidateDirection = dy > 0 ? 'down' : 'up';
      deviation = Math.abs(dx) / Math.max(1, Math.abs(dy));
    }
    if (candidateDirection !== direction) {
      continue;
    }
    const score: [number, number, number] = [
      edgeGap(source, candidate, direction),
      deviation,
      dx * dx + dy * dy,
    ];
    if (!best || compareScores(score, best.score) < 0) {
      best = { node: candidate, score };
    }
  }
  return best?.node ?? null;
}

function compareScores(
  left: [number, number, number],
  right: [number, number, number],
): number {
  for (let index = 0; index < left.length; index += 1) {
    const delta = left[index] - right[index];
    if (delta !== 0) {
      return delta;
    }
  }
  return 0;
}

export function computeNeighbors(nodes: DisplayNode[], nodeId: string): NeighborMap {
  const result: NeighborMap = {
    left: null,
    right: null,
    up: null,
    down: null,
  };
  const source = nodes.find((node) => node.id === nodeId);
  if (!source) {
    return result;
  }
  for (const direction of LAYOUT_DIRECTIONS) {
    const candidate = bestInDirection(source, nodes, direction);
    if (!candidate) {
      continue;
    }
    const back = bestInDirection(candidate, nodes, OPPOSITE_DIRECTION[direction]);
    if (back?.id === source.id) {
      result[direction] = candidate;
    }
  }
  return result;
}

export function primaryNodeForDevice(nodes: DisplayNode[], deviceId: string | null): DisplayNode | null {
  if (!deviceId) {
    return nodes.find((node) => node.is_primary) ?? nodes[0] ?? null;
  }
  const candidates = nodes.filter((node) => node.device_id === deviceId);
  if (candidates.length === 0) {
    return null;
  }
  return candidates.find((node) => node.is_primary) ?? candidates[0] ?? null;
}

export function fallbackDisplayNode(nodes: DisplayNode[], deviceId: string | null): DisplayNode | null {
  return (
    primaryNodeForDevice(nodes, deviceId) ??
    nodes.find((node) => node.is_local) ??
    nodes[0] ??
    null
  );
}
