import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react';
import type { DisplayNode } from '../api/types';
import { deviceColorClass } from '../domain/deviceColor';
import {
  CARD_GAP,
  CARD_HEIGHT,
  CARD_WIDTH,
  computeGridLayout,
  computeViewTransform,
  MIN_READABLE_SCALE,
  packDisplayFramesWithView,
  resolveGridPlacement,
  zoomViewAt,
  type ViewTransform,
} from '../domain/layoutGrid';
import { useApp } from '../state/appStore';
import { Icon } from './Icon';

interface DragState {
  nodeId: string;
  pointerId: number;
  offsetX: number;
  offsetY: number;
}

export function LayoutCanvas({
  nodes,
  selectedId,
  currentNodeId = null,
  revision,
  fitRequest = 0,
  deviceColorByDevice,
  onSelect,
  onNodeMove,
  onDragStart,
  onDragEnd,
}: {
  nodes: DisplayNode[];
  selectedId: string | null;
  currentNodeId?: string | null;
  revision: number;
  fitRequest?: number;
  deviceColorByDevice?: Map<string, number>;
  onSelect: (nodeId: string) => void;
  onNodeMove: (nodeId: string, origin: { x: number; y: number }) => void;
  onDragStart?: () => void;
  onDragEnd?: () => void;
}) {
  const { t } = useApp();
  const grid = useMemo(() => computeGridLayout(nodes), [nodes]);
  const displayNodes = grid.nodes;
  const containerRef = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState({ width: 720, height: 380 });
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [dragPreview, setDragPreview] = useState<{ left: number; top: number } | null>(
    null,
  );
  const [snapPreview, setSnapPreview] = useState<{
    left: number;
    top: number;
    width: number;
    height: number;
  } | null>(null);
  const [snapReferenceId, setSnapReferenceId] = useState<string | null>(null);
  const pendingPlacementRef = useRef<{ x: number; y: number } | null>(null);
  const dragRef = useRef<DragState | null>(null);
  const [panning, setPanning] = useState(false);
  const panRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    offsetX: number;
    offsetY: number;
  } | null>(null);
  const displayNodesRef = useRef(displayNodes);
  displayNodesRef.current = displayNodes;
  const gridRef = useRef(grid);
  gridRef.current = grid;
  const nodesRef = useRef(nodes);
  nodesRef.current = nodes;
  const viewportRef = useRef(viewport);
  viewportRef.current = viewport;
  const [view, setView] = useState<ViewTransform>(() =>
    computeViewTransform(displayNodes, 720, 380, MIN_READABLE_SCALE),
  );
  const viewRef = useRef(view);
  viewRef.current = view;
  const zoomAdjustedRef = useRef(false);
  const lastNodeKeyRef = useRef<string | null>(null);
  const lastFitRequestRef = useRef(fitRequest);
  const nodeKey = useMemo(
    () => nodes.map((node) => node.id).sort().join('|'),
    [nodes],
  );
  const primaryNodeByDevice = useMemo(() => {
    const result = new Map<string, string>();
    for (const node of nodes) {
      if (node.is_primary || !result.has(node.device_id)) {
        result.set(node.device_id, node.id);
      }
    }
    return result;
  }, [nodes]);

  useEffect(() => {
    const element = containerRef.current;
    if (!element) {
      return;
    }
    const measure = () => {
      const bounds = element.getBoundingClientRect();
      setViewport({
        width: Math.max(1, bounds.width),
        height: Math.max(1, bounds.height),
      });
    };
    measure();
    if (typeof ResizeObserver === 'undefined') {
      return;
    }
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [nodes.length]);

  useEffect(() => {
    if (displayNodesRef.current.length === 0 || zoomAdjustedRef.current) {
      return;
    }
    setView(
      computeViewTransform(
        displayNodesRef.current,
        viewportRef.current.width,
        viewportRef.current.height,
        MIN_READABLE_SCALE,
      ),
    );
  }, [viewport.width, viewport.height]);

  useEffect(() => {
    if (lastNodeKeyRef.current === nodeKey) {
      return;
    }
    lastNodeKeyRef.current = nodeKey;
    if (zoomAdjustedRef.current || displayNodesRef.current.length === 0) {
      return;
    }
    setView(
      computeViewTransform(
        displayNodesRef.current,
        viewportRef.current.width,
        viewportRef.current.height,
        MIN_READABLE_SCALE,
      ),
    );
  }, [nodeKey]);

  useEffect(() => {
    if (zoomAdjustedRef.current || displayNodesRef.current.length === 0) {
      return;
    }
    const fitted = computeViewTransform(
      displayNodesRef.current,
      viewportRef.current.width,
      viewportRef.current.height,
      MIN_READABLE_SCALE,
    );
    if (fitted.scale < viewRef.current.scale) {
      setView(fitted);
    }
  }, [revision]);

  useEffect(() => {
    if (fitRequest === lastFitRequestRef.current) {
      return;
    }
    lastFitRequestRef.current = fitRequest;
    if (displayNodesRef.current.length === 0) {
      return;
    }
    zoomAdjustedRef.current = false;
    setView(
      computeViewTransform(
        displayNodesRef.current,
        viewportRef.current.width,
        viewportRef.current.height,
      ),
    );
  }, [fitRequest]);

  useEffect(() => {
    const element = containerRef.current;
    if (!element) {
      return;
    }
    const handleWheel = (event: WheelEvent) => {
      if (!event.ctrlKey) {
        return;
      }
      event.preventDefault();
      const bounds = element.getBoundingClientRect();
      const pointer = {
        x: event.clientX - bounds.left,
        y: event.clientY - bounds.top,
      };
      const factor = Math.exp(-event.deltaY * 0.0015);
      const next = zoomViewAt(viewRef.current, pointer, factor);
      if (next === viewRef.current) {
        return;
      }
      viewRef.current = next;
      zoomAdjustedRef.current = true;
      setView(next);
    };
    element.addEventListener('wheel', handleWheel, { passive: false });
    return () => element.removeEventListener('wheel', handleWheel);
  }, [nodes.length]);

  const anchorId = grid.anchorId;

  const frames = useMemo(
    () => packDisplayFramesWithView(displayNodes, view),
    [displayNodes, view, revision],
  );

  const placeNodeAtDisplayPoint = (
    nodeId: string,
    targetCenter: { x: number; y: number },
  ) => {
    const placement = resolveGridPlacement(
      gridRef.current,
      nodesRef.current,
      nodeId,
      targetCenter,
    );
    const current = nodesRef.current.find((node) => node.id === nodeId);
    if (!placement || !current) {
      return;
    }
    const x = Math.round(placement.origin.x);
    const y = Math.round(placement.origin.y);
    if (current.origin.x === x && current.origin.y === y) {
      return;
    }
    onNodeMove(nodeId, { x, y });
  };

  const handlePointerDown = (
    event: ReactPointerEvent<HTMLDivElement>,
    node: DisplayNode,
  ) => {
    if (event.button !== 0) {
      return;
    }
    const frame = frames.get(node.id);
    if (!frame) {
      return;
    }
    event.stopPropagation();
    event.preventDefault();
    event.currentTarget.focus();
    onSelect(node.id);
    if (node.id === anchorId) {
      return;
    }
    onDragStart?.();
    event.currentTarget.setPointerCapture(event.pointerId);
    const container = containerRef.current?.getBoundingClientRect();
    const pointerX = event.clientX - (container?.left ?? 0);
    const pointerY = event.clientY - (container?.top ?? 0);
    dragRef.current = {
      nodeId: node.id,
      pointerId: event.pointerId,
      offsetX: pointerX - frame.left,
      offsetY: pointerY - frame.top,
    };
    pendingPlacementRef.current = null;
    setSnapPreview(null);
    setSnapReferenceId(null);
    setDraggingId(node.id);
    setDragPreview({ left: frame.left, top: frame.top });
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) {
      return;
    }
    const container = containerRef.current?.getBoundingClientRect();
    const frame = frames.get(drag.nodeId);
    const cardWidth = frame?.width ?? CARD_WIDTH * viewRef.current.scale;
    const cardHeight = frame?.height ?? CARD_HEIGHT * viewRef.current.scale;
    if (container) {
      const maxLeft = Math.max(0, container.width - cardWidth);
      const maxTop = Math.max(0, container.height - cardHeight);
      setDragPreview({
        left: Math.min(
          maxLeft,
          Math.max(0, event.clientX - container.left - drag.offsetX),
        ),
        top: Math.min(
          maxTop,
          Math.max(0, event.clientY - container.top - drag.offsetY),
        ),
      });
    }

    const view = viewRef.current;
    const scale = Math.max(0.001, view.scale);
    const pointerScreenX = event.clientX - (container?.left ?? 0);
    const pointerScreenY = event.clientY - (container?.top ?? 0);
    const placement = resolveGridPlacement(
      gridRef.current,
      nodesRef.current,
      drag.nodeId,
      {
        x: (pointerScreenX - view.offsetX) / scale,
        y: (pointerScreenY - view.offsetY) / scale,
      },
    );
    if (!placement) {
      pendingPlacementRef.current = null;
      setSnapPreview(null);
      setSnapReferenceId(null);
      return;
    }
    pendingPlacementRef.current = placement.origin;
    const draggedDisplay = gridRef.current.byId.get(drag.nodeId);
    if (!draggedDisplay) {
      setSnapPreview(null);
      setSnapReferenceId(null);
      return;
    }
    setSnapPreview({
      left: placement.displayOrigin.x * scale + view.offsetX,
      top: placement.displayOrigin.y * scale + view.offsetY,
      width: draggedDisplay.resolution.width * scale,
      height: draggedDisplay.resolution.height * scale,
    });
    setSnapReferenceId(placement.referenceId);
  };

  const commitPendingPlacement = (nodeId: string) => {
    const pending = pendingPlacementRef.current;
    pendingPlacementRef.current = null;
    if (!pending) {
      return;
    }
    const current = nodesRef.current.find((node) => node.id === nodeId);
    if (
      current &&
      (current.origin.x !== pending.x || current.origin.y !== pending.y)
    ) {
      onNodeMove(nodeId, pending);
    }
  };

  const finishDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) {
      return;
    }
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    dragRef.current = null;
    commitPendingPlacement(drag.nodeId);
    setDraggingId(null);
    setDragPreview(null);
    setSnapPreview(null);
    setSnapReferenceId(null);
    onDragEnd?.();
  };

  const cancelDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) {
      return;
    }
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    dragRef.current = null;
    pendingPlacementRef.current = null;
    setDraggingId(null);
    setDragPreview(null);
    setSnapPreview(null);
    setSnapReferenceId(null);
    onDragEnd?.();
  };

  const handleCanvasPointerDown = (
    event: ReactPointerEvent<HTMLDivElement>,
  ) => {
    if (event.button !== 0 && event.button !== 1) {
      return;
    }
    if (
      event.button === 0 &&
      (event.target as Element).closest('.layout-node') !== null
    ) {
      return;
    }
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    panRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      offsetX: viewRef.current.offsetX,
      offsetY: viewRef.current.offsetY,
    };
    zoomAdjustedRef.current = true;
    setPanning(true);
  };

  const handleCanvasPointerMove = (
    event: ReactPointerEvent<HTMLDivElement>,
  ) => {
    const pan = panRef.current;
    if (!pan || pan.pointerId !== event.pointerId) {
      return;
    }
    const next: ViewTransform = {
      ...viewRef.current,
      offsetX: pan.offsetX + (event.clientX - pan.startX),
      offsetY: pan.offsetY + (event.clientY - pan.startY),
    };
    viewRef.current = next;
    setView(next);
  };

  const finishPan = (event: ReactPointerEvent<HTMLDivElement>) => {
    const pan = panRef.current;
    if (!pan || pan.pointerId !== event.pointerId) {
      return;
    }
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    panRef.current = null;
    setPanning(false);
  };

  const handleNodeKeyDown = (
    event: ReactKeyboardEvent<HTMLDivElement>,
    node: DisplayNode,
  ) => {
    const displayNode = grid.byId.get(node.id);
    if (!displayNode) {
      return;
    }
    const factor = event.shiftKey ? 2 : 1;
    const stepX = (displayNode.resolution.width + CARD_GAP) * factor;
    const stepY = (displayNode.resolution.height + CARD_GAP) * factor;
    let deltaX = 0;
    let deltaY = 0;
    if (event.key === 'ArrowLeft') {
      deltaX = -stepX;
    } else if (event.key === 'ArrowRight') {
      deltaX = stepX;
    } else if (event.key === 'ArrowUp') {
      deltaY = -stepY;
    } else if (event.key === 'ArrowDown') {
      deltaY = stepY;
    } else if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      onSelect(node.id);
      return;
    } else {
      return;
    }
    event.preventDefault();
    onSelect(node.id);
    if (node.id === anchorId) {
      return;
    }
    placeNodeAtDisplayPoint(node.id, {
      x: displayNode.origin.x + displayNode.resolution.width / 2 + deltaX,
      y: displayNode.origin.y + displayNode.resolution.height / 2 + deltaY,
    });
  };

  if (nodes.length === 0) {
    return (
      <div className="layout-canvas layout-empty">
        <span>{t('layout.empty')}</span>
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      className={panning ? 'layout-canvas is-panning' : 'layout-canvas'}
      aria-label={t('layout.canvasAria')}
      onPointerDown={handleCanvasPointerDown}
      onPointerMove={handleCanvasPointerMove}
      onPointerUp={finishPan}
      onPointerCancel={finishPan}
    >
      {snapPreview ? (
        <div
          className="layout-snap-ghost"
          style={{
            left: `${snapPreview.left}px`,
            top: `${snapPreview.top}px`,
            width: `${snapPreview.width}px`,
            height: `${snapPreview.height}px`,
          }}
          aria-hidden="true"
        />
      ) : null}
      {nodes.map((node) => {
        const packed = frames.get(node.id);
        if (!packed) {
          return null;
        }
        const selected = selectedId === node.id;
        const dragging = draggingId === node.id;
        const displayName =
          node.display_name.trim() ||
          t('layout.monitorIndex', { index: node.monitor_index + 1 });
        const resolutionLabel = `${node.resolution.width}×${node.resolution.height}`;
        const frame =
          dragging && dragPreview
            ? { ...packed, left: dragPreview.left, top: dragPreview.top }
            : packed;
        return (
          <div
            key={node.id}
            role="button"
            tabIndex={0}
            className={[
              'layout-node',
              deviceColorClass(deviceColorByDevice?.get(node.device_id)),
              selected ? 'is-selected' : '',
              dragging ? 'is-dragging' : '',
              node.id === anchorId ? 'is-anchor' : '',
              node.id === currentNodeId ? 'is-current' : '',
              node.id === snapReferenceId ? 'is-snap-reference' : '',
              node.is_local ? 'is-local' : 'is-remote',
            ]
              .filter(Boolean)
              .join(' ')}
            style={{
              left: `${frame.left}px`,
              top: `${frame.top}px`,
              width: `${frame.width}px`,
              height: `${frame.height}px`,
            }}
            title={`${node.device_name} · ${displayName} · ${resolutionLabel}`}
            aria-label={t('layout.nodeAria', {
              name: node.device_name,
              index: node.monitor_index + 1,
            })}
            aria-pressed={selected}
            aria-disabled={node.id === anchorId}
            onClick={() => onSelect(node.id)}
            onKeyDown={(event) => handleNodeKeyDown(event, node)}
            onPointerDown={(event) => handlePointerDown(event, node)}
            onPointerMove={handlePointerMove}
            onPointerUp={finishDrag}
            onPointerCancel={cancelDrag}
          >
            <div className="layout-node-head">
              <span className="layout-node-icon" aria-hidden="true">
                <Icon name="monitor" size={14} />
              </span>
              <span className="device-dot" aria-hidden="true" />
              {primaryNodeByDevice.get(node.device_id) === node.id ? (
                <strong>{node.device_name}</strong>
              ) : null}
              {node.id === currentNodeId ? (
                <span className="layout-node-tag is-current">
                  {t('layout.currentControl')}
                </span>
              ) : null}
              <span className="layout-node-tag">#{node.monitor_index + 1}</span>
            </div>
            <span className="layout-node-display">{displayName}</span>
            <span className="layout-node-resolution">
              {resolutionLabel}
              {node.resolution.height > node.resolution.width
                ? ` · ${t('layout.portrait')}`
                : ''}
            </span>
            <span className="layout-node-grip" aria-hidden="true">
              <i />
              <i />
              <i />
              <i />
              <i />
              <i />
            </span>
          </div>
        );
      })}
    </div>
  );
}
