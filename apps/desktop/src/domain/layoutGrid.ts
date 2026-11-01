import type { DisplayNode } from '../api/types';
import { computeNeighbors, LAYOUT_DIRECTIONS, type LayoutDirection } from './layout';

export const CANVAS_PADDING = 30;
export const CARD_WIDTH = 1920;
export const CARD_HEIGHT = 1080;
export const CARD_GAP = 24;
export const MIN_READABLE_SCALE = 0.1;
export const MAX_VIEW_SCALE = 0.16;
export const MIN_ZOOM_SCALE = 0.02;
export const MAX_ZOOM_SCALE = 0.6;

export function cardSize(node: DisplayNode): { width: number; height: number } {
  return node.resolution.height > node.resolution.width
    ? { width: CARD_HEIGHT, height: CARD_WIDTH }
    : { width: CARD_WIDTH, height: CARD_HEIGHT };
}

export interface GridFrame {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface ViewTransform {
  scale: number;
  offsetX: number;
  offsetY: number;
}

export interface GridCell {
  col: number;
  row: number;
}

export interface GridLayout {
  nodes: DisplayNode[];
  byId: Map<string, DisplayNode>;
  cells: Map<string, GridCell>;
  anchorId: string | null;
}

const CELL_STEP: Record<LayoutDirection, GridCell> = {
  left: { col: -1, row: 0 },
  right: { col: 1, row: 0 },
  up: { col: 0, row: -1 },
  down: { col: 0, row: 1 },
};

function armAnchor(nodes: DisplayNode[]): DisplayNode | null {
  return (
    nodes.find((node) => node.is_local && node.is_primary) ??
    nodes.find((node) => node.is_local) ??
    nodes[0] ??
    null
  );
}

function stepCell(cell: GridCell, direction: LayoutDirection): GridCell {
  const step = CELL_STEP[direction];
  return { col: cell.col + step.col, row: cell.row + step.row };
}

function displayCenter(node: DisplayNode): { x: number; y: number } {
  return {
    x: node.origin.x + node.resolution.width / 2,
    y: node.origin.y + node.resolution.height / 2,
  };
}

function directionFromDelta(
  dx: number,
  dy: number,
  reference: DisplayNode,
  dragged: DisplayNode,
): LayoutDirection {
  const halfWidth = Math.max(
    1,
    (reference.resolution.width + dragged.resolution.width) / 2,
  );
  const halfHeight = Math.max(
    1,
    (reference.resolution.height + dragged.resolution.height) / 2,
  );
  const normalizedX = dx / halfWidth;
  const normalizedY = dy / halfHeight;
  if (Math.abs(normalizedX) >= Math.abs(normalizedY)) {
    return normalizedX >= 0 ? 'right' : 'left';
  }
  return normalizedY >= 0 ? 'down' : 'up';
}

export function computeGridLayout(nodes: DisplayNode[]): GridLayout {
  const anchor = armAnchor(nodes);
  if (!anchor) {
    return {
      nodes: [],
      byId: new Map(),
      cells: new Map(),
      anchorId: null,
    };
  }

  const neighborMap = new Map(
    nodes.map((node) => [node.id, computeNeighbors(nodes, node.id)]),
  );
  const cells = new Map<string, GridCell>([[anchor.id, { col: 0, row: 0 }]]);
  const queue = [anchor.id];
  let maxCol = 0;
  while (queue.length > 0) {
    const currentId = queue.shift()!;
    const current = cells.get(currentId);
    const neighbors = neighborMap.get(currentId);
    if (!current || !neighbors) {
      continue;
    }
    for (const direction of LAYOUT_DIRECTIONS) {
      const neighbor = neighbors[direction];
      if (!neighbor || cells.has(neighbor.id)) {
        continue;
      }
      const cell = stepCell(current, direction);
      cells.set(neighbor.id, cell);
      maxCol = Math.max(maxCol, cell.col);
      queue.push(neighbor.id);
    }
  }

  let detachedCol = maxCol + 2;
  for (const node of nodes) {
    if (cells.has(node.id)) {
      continue;
    }
    cells.set(node.id, { col: detachedCol, row: 0 });
    detachedCol += 1;
  }

  const sizes = new Map(nodes.map((node) => [node.id, cardSize(node)]));
  const colWidths = new Map<number, number>();
  const rowHeights = new Map<number, number>();
  for (const node of nodes) {
    const cell = cells.get(node.id)!;
    const size = sizes.get(node.id)!;
    colWidths.set(cell.col, Math.max(colWidths.get(cell.col) ?? 0, size.width));
    rowHeights.set(
      cell.row,
      Math.max(rowHeights.get(cell.row) ?? 0, size.height),
    );
  }
  const colX = new Map<number, number>();
  let nextX = 0;
  for (const col of [...colWidths.keys()].sort((left, right) => left - right)) {
    colX.set(col, nextX);
    nextX += (colWidths.get(col) ?? 0) + CARD_GAP;
  }

  const rowMembers = new Map<number, DisplayNode[]>();
  for (const node of nodes) {
    const row = cells.get(node.id)!.row;
    const members = rowMembers.get(row);
    if (members) {
      members.push(node);
    } else {
      rowMembers.set(row, [node]);
    }
  }
  const rowPositions = new Map<number, Map<string, number>>();
  const rowDisplayHeights = new Map<number, number>();
  for (const [row, members] of rowMembers) {
    const positions = new Map<string, number>();
    for (const node of members) {
      positions.set(node.id, 0);
    }
    rowPositions.set(row, positions);
    rowDisplayHeights.set(row, Math.max(1, rowHeights.get(row) ?? 0));
  }

  const rowY = new Map<number, number>();
  let nextY = 0;
  for (const row of [...rowDisplayHeights.keys()].sort((left, right) => left - right)) {
    rowY.set(row, nextY);
    nextY += (rowDisplayHeights.get(row) ?? 0) + CARD_GAP;
  }

  const displayNodes = nodes.map((node) => {
    const cell = cells.get(node.id)!;
    const size = sizes.get(node.id)!;
    const cellX = colX.get(cell.col)!;
    const cellY = rowY.get(cell.row)!;
    const rowPosition = rowPositions.get(cell.row)?.get(node.id) ?? 0;
    return {
      ...node,
      origin: {
        x: cellX + ((colWidths.get(cell.col) ?? size.width) - size.width) / 2,
        y: cellY + rowPosition,
      },
      resolution: size,
    };
  });
  return {
    nodes: displayNodes,
    byId: new Map(displayNodes.map((node) => [node.id, node])),
    cells,
    anchorId: anchor.id,
  };
}

export interface GridPlacement {
  cell: GridCell;
  direction: LayoutDirection;
  referenceId: string;
  origin: { x: number; y: number };
  displayOrigin: { x: number; y: number };
}

const MIN_ALONG_OVERLAP_RATIO = 0.25;
const MIN_ALONG_OVERLAP_PX = 24;

function clampAlong(
  desired: number,
  referenceStart: number,
  referenceSize: number,
  draggedSize: number,
): number {
  const overlap = Math.max(
    MIN_ALONG_OVERLAP_PX,
    Math.min(referenceSize, draggedSize) * MIN_ALONG_OVERLAP_RATIO,
  );
  const lower = referenceStart + overlap - draggedSize / 2;
  const upper = referenceStart + referenceSize - overlap + draggedSize / 2;
  if (lower > upper) {
    return (lower + upper) / 2;
  }
  return Math.min(upper, Math.max(lower, desired));
}

function placeBeside(
  reference: DisplayNode,
  dragged: DisplayNode,
  direction: LayoutDirection,
  desiredCenter: number,
): { x: number; y: number } {
  const bounds = {
    x: reference.origin.x,
    y: reference.origin.y,
    right: reference.origin.x + reference.resolution.width,
    bottom: reference.origin.y + reference.resolution.height,
  };
  const width = dragged.resolution.width;
  const height = dragged.resolution.height;
  switch (direction) {
    case 'right': {
      const centerY = clampAlong(
        desiredCenter,
        bounds.y,
        reference.resolution.height,
        height,
      );
      return {
        x: Math.round(bounds.right + CARD_GAP),
        y: Math.round(centerY - height / 2),
      };
    }
    case 'left': {
      const centerY = clampAlong(
        desiredCenter,
        bounds.y,
        reference.resolution.height,
        height,
      );
      return {
        x: Math.round(bounds.x - CARD_GAP - width),
        y: Math.round(centerY - height / 2),
      };
    }
    case 'down': {
      const centerX = clampAlong(
        desiredCenter,
        bounds.x,
        reference.resolution.width,
        width,
      );
      return {
        x: Math.round(centerX - width / 2),
        y: Math.round(bounds.bottom + CARD_GAP),
      };
    }
    case 'up': {
      const centerX = clampAlong(
        desiredCenter,
        bounds.x,
        reference.resolution.width,
        width,
      );
      return {
        x: Math.round(centerX - width / 2),
        y: Math.round(bounds.y - CARD_GAP - height),
      };
    }
  }
}

function squaredDistance(
  left: { x: number; y: number },
  right: { x: number; y: number },
): number {
  return (left.x - right.x) ** 2 + (left.y - right.y) ** 2;
}

function pointRectDistance(
  point: { x: number; y: number },
  node: DisplayNode,
): number {
  const left = node.origin.x;
  const right = left + node.resolution.width;
  const top = node.origin.y;
  const bottom = top + node.resolution.height;
  const dx = point.x < left ? left - point.x : point.x > right ? point.x - right : 0;
  const dy = point.y < top ? top - point.y : point.y > bottom ? point.y - bottom : 0;
  return Math.hypot(dx, dy);
}

function originOverlaps(
  origin: { x: number; y: number },
  size: { width: number; height: number },
  node: DisplayNode,
): boolean {
  return (
    origin.x < node.origin.x + node.resolution.width &&
    node.origin.x < origin.x + size.width &&
    origin.y < node.origin.y + node.resolution.height &&
    node.origin.y < origin.y + size.height
  );
}

export function resolveGridPlacement(
  grid: GridLayout,
  nodes: DisplayNode[],
  nodeId: string,
  targetPoint: { x: number; y: number },
): GridPlacement | null {
  const dragged = nodes.find((node) => node.id === nodeId);
  if (!dragged) {
    return null;
  }

  const candidates: DisplayNode[] = grid.nodes.filter(
    (node) => node.id !== nodeId && grid.cells.has(node.id),
  );
  if (candidates.length === 0) {
    return null;
  }

  let reference = candidates[0]!;
  let bestRectDistance = pointRectDistance(targetPoint, reference);
  let bestCenterDistance = squaredDistance(targetPoint, displayCenter(reference));
  for (const candidate of candidates.slice(1)) {
    const rectDistance = pointRectDistance(targetPoint, candidate);
    const centerDistance = squaredDistance(targetPoint, displayCenter(candidate));
    if (
      rectDistance < bestRectDistance ||
      (rectDistance === bestRectDistance && centerDistance < bestCenterDistance)
    ) {
      reference = candidate;
      bestRectDistance = rectDistance;
      bestCenterDistance = centerDistance;
    }
  }

  const referenceCell = grid.cells.get(reference.id)!;
  const draggedDisplay = grid.byId.get(nodeId) ?? dragged;
  const direction = directionFromDelta(
    targetPoint.x - displayCenter(reference).x,
    targetPoint.y - displayCenter(reference).y,
    reference,
    draggedDisplay,
  );
  const destCell = stepCell(referenceCell, direction);
  const horizontal = direction === 'left' || direction === 'right';
  const realReference = nodes.find((node) => node.id === reference.id) ?? reference;
  const desiredCenter = horizontal
    ? realReference.origin.y +
      realReference.resolution.height / 2 +
      (targetPoint.y - displayCenter(reference).y) *
        (realReference.resolution.height /
          Math.max(1, reference.resolution.height))
    : realReference.origin.x +
      realReference.resolution.width / 2 +
      (targetPoint.x - displayCenter(reference).x) *
        (realReference.resolution.width /
          Math.max(1, reference.resolution.width));
  let finalReference = realReference;
  let origin = placeBeside(finalReference, dragged, direction, desiredCenter);
  for (let attempt = 0; attempt < nodes.length; attempt += 1) {
    const occupant = nodes.find(
      (node) =>
        node.id !== nodeId &&
        node.id !== finalReference.id &&
        originOverlaps(origin, dragged.resolution, node),
    );
    if (!occupant) {
      break;
    }
    finalReference = occupant;
    origin = placeBeside(finalReference, dragged, direction, desiredCenter);
  }
  if (
    origin.x === Math.round(dragged.origin.x) &&
    origin.y === Math.round(dragged.origin.y)
  ) {
    return null;
  }

  const finalDisplay = grid.byId.get(finalReference.id) ?? reference;
  const displayAlong = horizontal
    ? clampAlong(
        targetPoint.y,
        finalDisplay.origin.y,
        finalDisplay.resolution.height,
        draggedDisplay.resolution.height,
      )
    : clampAlong(
        targetPoint.x,
        finalDisplay.origin.x,
        finalDisplay.resolution.width,
        draggedDisplay.resolution.width,
      );
  const displayOrigin = horizontal
    ? {
        x:
          direction === 'right'
            ? finalDisplay.origin.x + finalDisplay.resolution.width + CARD_GAP
            : finalDisplay.origin.x - CARD_GAP - draggedDisplay.resolution.width,
        y: displayAlong - draggedDisplay.resolution.height / 2,
      }
    : {
        x: displayAlong - draggedDisplay.resolution.width / 2,
        y:
          direction === 'down'
            ? finalDisplay.origin.y + finalDisplay.resolution.height + CARD_GAP
            : finalDisplay.origin.y - CARD_GAP - draggedDisplay.resolution.height,
      };

  return {
    cell: destCell,
    direction,
    referenceId: finalReference.id,
    origin,
    displayOrigin,
  };
}

export function computeViewTransform(
  nodes: DisplayNode[],
  width: number,
  height: number,
  minScale = 0.01,
): ViewTransform {
  if (nodes.length === 0 || width <= 0 || height <= 0) {
    return { scale: 0.1, offsetX: 0, offsetY: 0 };
  }
  const minX = Math.min(...nodes.map((node) => node.origin.x));
  const minY = Math.min(...nodes.map((node) => node.origin.y));
  const maxX = Math.max(...nodes.map((node) => node.origin.x + node.resolution.width));
  const maxY = Math.max(...nodes.map((node) => node.origin.y + node.resolution.height));
  const anchor =
    nodes.find((node) => node.is_local && node.is_primary) ??
    nodes.find((node) => node.is_local) ??
    null;
  const pivotX = anchor
    ? anchor.origin.x + anchor.resolution.width / 2
    : (minX + maxX) / 2;
  const pivotY = anchor
    ? anchor.origin.y + anchor.resolution.height / 2
    : (minY + maxY) / 2;
  const halfWidth = Math.max(1, pivotX - minX, maxX - pivotX);
  const halfHeight = Math.max(1, pivotY - minY, maxY - pivotY);
  const availableHalfWidth = Math.max(60, (width - CANVAS_PADDING * 2) / 2);
  const availableHalfHeight = Math.max(50, (height - CANVAS_PADDING * 2) / 2);
  const fitted = Math.min(
    availableHalfWidth / halfWidth,
    availableHalfHeight / halfHeight,
    MAX_VIEW_SCALE,
  );
  const scale = Math.max(0.01, fitted, Math.min(minScale, MAX_VIEW_SCALE));
  return {
    scale,
    offsetX: width / 2 - pivotX * scale,
    offsetY: height / 2 - pivotY * scale,
  };
}

export function zoomViewAt(
  view: ViewTransform,
  pointer: { x: number; y: number },
  factor: number,
): ViewTransform {
  const scale = Math.min(
    MAX_ZOOM_SCALE,
    Math.max(MIN_ZOOM_SCALE, view.scale * factor),
  );
  if (!Number.isFinite(scale) || scale === view.scale) {
    return view;
  }
  const worldX = (pointer.x - view.offsetX) / view.scale;
  const worldY = (pointer.y - view.offsetY) / view.scale;
  return {
    scale,
    offsetX: pointer.x - worldX * scale,
    offsetY: pointer.y - worldY * scale,
  };
}

export function packDisplayFramesWithView(
  nodes: DisplayNode[],
  view: ViewTransform,
): Map<string, GridFrame> {
  const result = new Map<string, GridFrame>();
  for (const node of nodes) {
    result.set(node.id, {
      left: node.origin.x * view.scale + view.offsetX,
      top: node.origin.y * view.scale + view.offsetY,
      width: node.resolution.width * view.scale,
      height: node.resolution.height * view.scale,
    });
  }
  return result;
}

export function packDisplayFrames(
  nodes: DisplayNode[],
  width: number,
  height: number,
): Map<string, GridFrame> {
  return packDisplayFramesWithView(
    nodes,
    computeViewTransform(nodes, width, height),
  );
}
