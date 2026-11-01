import { useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../api/client';
import type {
  DisplayNode,
  ScreenOrientation,
  TargetFormat,
} from '../api/types';
import { buildDeviceGroups } from '../domain/deviceColor';
import {
  FPS_PRESETS,
  RESOLUTION_PRESETS,
  findMatchingFps,
  findMatchingResolution,
  recommendedBitrate,
} from '../domain/presets';
import { useApp } from '../state/appStore';

export function preferredSelection(
  nodes: DisplayNode[],
  activeIds: string[],
  current: string | null,
): string | null {
  const active = new Set(activeIds);
  const visible = nodes.filter((node) => active.has(node.id));
  if (current && visible.some((node) => node.id === current)) {
    return current;
  }
  return visible.find((node) => node.is_local)?.id ?? visible[0]?.id ?? null;
}

function sameMetadata(left: DisplayNode, right: DisplayNode): boolean {
  return (
    left.device_id === right.device_id &&
    left.device_name === right.device_name &&
    left.display_name === right.display_name &&
    left.monitor_index === right.monitor_index &&
    left.resolution.width === right.resolution.width &&
    left.resolution.height === right.resolution.height &&
    left.dpi === right.dpi &&
    left.physical.width_mm === right.physical.width_mm &&
    left.physical.height_mm === right.physical.height_mm &&
    left.is_local === right.is_local &&
    left.is_primary === right.is_primary
  );
}

export function useLayoutDraft() {
  const { state, t, reportError, applyLayout, saveSettings } = useApp();
  const [draft, setDraft] = useState<DisplayNode[]>(state.layout.nodes);
  const [selectedId, setSelectedId] = useState<string | null>(() =>
    preferredSelection(state.layout.nodes, state.layout.active_node_ids, null),
  );
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [fitRequest, setFitRequest] = useState(0);
  const [historyCount, setHistoryCount] = useState(0);
  const historyRef = useRef<
    Array<{
      nodes: DisplayNode[];
      overrides: Record<string, ScreenOrientation>;
    }>
  >([]);
  const draggingRef = useRef(false);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const draftVersionRef = useRef(0);
  const lastSavedVersionRef = useRef(0);
  const queuedVersionRef = useRef(0);
  const revisionRef = useRef(state.layout.revision);
  const activeNodeIdsRef = useRef(state.layout.active_node_ids);
  activeNodeIdsRef.current = state.layout.active_node_ids;
  const overridesRef = useRef(state.layout.orientation_overrides ?? {});
  const saveQueueRef = useRef<Promise<void>>(Promise.resolve());
  const pendingSavesRef = useRef(0);

  useEffect(() => {
    revisionRef.current = state.layout.revision;
  }, [state.layout.revision]);

  useEffect(() => {
    if (!dirty) {
      overridesRef.current = state.layout.orientation_overrides ?? {};
    }
  }, [state.layout.orientation_overrides, dirty]);

  useEffect(() => {
    if (!dirty) {
      setDraft(state.layout.nodes);
      setSelectedId((current) =>
        preferredSelection(state.layout.nodes, state.layout.active_node_ids, current),
      );
      return;
    }
    const current = draftRef.current;
    const externalById = new Map(state.layout.nodes.map((node) => [node.id, node]));
    let mergedChanged = false;
    const merged = current.map((node) => {
      const external = externalById.get(node.id);
      if (!external) {
        return node;
      }
      if (sameMetadata(node, external)) {
        return node;
      }
      mergedChanged = true;
      const sizeChanged =
        node.resolution.width !== external.resolution.width ||
        node.resolution.height !== external.resolution.height;
      return sizeChanged ? external : { ...external, origin: node.origin };
    });
    const known = new Set(current.map((node) => node.id));
    for (const node of state.layout.nodes) {
      if (!known.has(node.id)) {
        merged.push(node);
        mergedChanged = true;
      }
    }
    if (mergedChanged) {
      draftRef.current = merged;
      draftVersionRef.current += 1;
      setDraft(merged);
    }
    setSelectedId((current) =>
      preferredSelection(
        state.layout.nodes,
        state.layout.active_node_ids,
        current,
      ),
    );
  }, [state.layout, dirty]);

  const pushHistory = () => {
    const nodes = draftRef.current.map((node) => ({
      ...node,
      resolution: { ...node.resolution },
      physical: { ...node.physical },
      origin: { ...node.origin },
    }));
    historyRef.current = [
      ...historyRef.current.slice(-19),
      { nodes, overrides: { ...overridesRef.current } },
    ];
    setHistoryCount(historyRef.current.length);
  };

  const persistDraft = (nodes: DisplayNode[], version: number) => {
    if (version <= lastSavedVersionRef.current || version <= queuedVersionRef.current) {
      return;
    }
    queuedVersionRef.current = version;
    pendingSavesRef.current += 1;
    const overrides = { ...overridesRef.current };
    setSaving(true);
    saveQueueRef.current = saveQueueRef.current.then(async () => {
      try {
        const saved = await api.setLayout({
          nodes,
          revision: revisionRef.current,
          active_node_ids: activeNodeIdsRef.current,
          orientation_overrides: overrides,
        });
        revisionRef.current = saved.revision;
        lastSavedVersionRef.current = Math.max(lastSavedVersionRef.current, version);
        applyLayout(saved);
        if (draftVersionRef.current === version) {
          overridesRef.current = saved.orientation_overrides ?? {};
          setDraft(saved.nodes);
          setDirty(false);
        }
      } catch (error) {
        reportError(error, t('layout.saveFailed'));
      } finally {
        pendingSavesRef.current -= 1;
        if (pendingSavesRef.current === 0) {
          setSaving(false);
        }
      }
    });
  };

  const undo = () => {
    const previous = historyRef.current.pop();
    if (!previous) {
      return;
    }
    setHistoryCount(historyRef.current.length);
    overridesRef.current = { ...previous.overrides };
    draftRef.current = previous.nodes;
    draftVersionRef.current += 1;
    setDraft(previous.nodes);
    setDirty(true);
    persistDraft(previous.nodes, draftVersionRef.current);
  };

  useEffect(() => {
    if (!dirty || dragging) {
      return;
    }
    const version = draftVersionRef.current;
    const timer = window.setTimeout(() => {
      persistDraft(draftRef.current, version);
    }, 350);
    return () => window.clearTimeout(timer);
  }, [draft, dirty, dragging]);

  const activeNodeIds = useMemo(
    () => new Set(state.layout.active_node_ids),
    [state.layout.active_node_ids],
  );
  const visibleNodes = useMemo(
    () => draft.filter((node) => activeNodeIds.has(node.id)),
    [draft, activeNodeIds],
  );
  const deviceGroups = useMemo(() => buildDeviceGroups(visibleNodes), [visibleNodes]);
  const deviceColorByDevice = useMemo(
    () => new Map(deviceGroups.map((group) => [group.deviceId, group.colorIndex])),
    [deviceGroups],
  );
  const selectedNode = useMemo(
    () =>
      draft.find((node) => node.id === selectedId && activeNodeIds.has(node.id)) ?? null,
    [draft, selectedId, activeNodeIds],
  );
  const selectedPortrait = Boolean(
    selectedNode && selectedNode.resolution.height > selectedNode.resolution.width,
  );

  const setOrientationFor = (nodeId: string, orientation: ScreenOrientation) => {
    const node = draftRef.current.find((candidate) => candidate.id === nodeId);
    if (!node) {
      return;
    }
    pushHistory();
    const portraitNow = node.resolution.height > node.resolution.width;
    const portraitNext = orientation === 'portrait';
    if (overridesRef.current[nodeId] === orientation && portraitNow === portraitNext) {
      return;
    }
    overridesRef.current = {
      ...overridesRef.current,
      [nodeId]: orientation,
    };
    const next = draftRef.current.map((candidate) => {
      if (candidate.id !== nodeId || portraitNow === portraitNext) {
        return candidate;
      }
      return {
        ...candidate,
        resolution: {
          width: candidate.resolution.height,
          height: candidate.resolution.width,
        },
        physical: {
          width_mm: candidate.physical.height_mm,
          height_mm: candidate.physical.width_mm,
        },
      };
    });
    draftRef.current = next;
    draftVersionRef.current += 1;
    setDraft(next);
    setDirty(true);
    persistDraft(next, draftVersionRef.current);
  };

  const setOrientation = (orientation: ScreenOrientation) => {
    if (selectedNode) {
      setOrientationFor(selectedNode.id, orientation);
    }
  };

  const selectedDeviceId = selectedNode?.device_id ?? null;
  const target: TargetFormat = selectedDeviceId
    ? state.settings.video_targets[selectedDeviceId] ?? state.settings.target
    : state.settings.target;
  const selectedResolution = findMatchingResolution(target);
  const selectedFps = findMatchingFps(target);
  const resolutionValue = selectedResolution?.id ?? 'custom';
  const fpsValue = selectedFps?.id ?? 'custom';
  const resolutionOptions = [
    ...RESOLUTION_PRESETS.map((preset) => ({
      value: preset.id,
      label: `${preset.width}×${preset.height}`,
    })),
    ...(selectedResolution
      ? []
      : [
          {
            value: 'custom',
            label: `${target.width}×${target.height}`,
          },
        ]),
  ];
  const fpsOptions = [
    ...FPS_PRESETS.map((preset) => ({
      value: preset.id,
      label: `${preset.fps} Hz`,
    })),
    ...(selectedFps ? [] : [{ value: 'custom', label: `${target.fps} Hz` }]),
  ];

  const persistTarget = (patch: Partial<TargetFormat>) => {
    if (!selectedDeviceId) {
      return;
    }
    const next: TargetFormat = { ...target, ...patch };
    next.bitrate_kbps = recommendedBitrate(next.width, next.height, next.fps);
    void saveSettings({
      video_targets: {
        ...state.settings.video_targets,
        [selectedDeviceId]: next,
      },
    });
  };

  const selectResolution = (value: string) => {
    const preset = RESOLUTION_PRESETS.find((item) => item.id === value);
    if (preset) {
      persistTarget({ width: preset.width, height: preset.height });
    }
  };

  const selectFps = (value: string) => {
    const preset = FPS_PRESETS.find((item) => item.id === value);
    if (preset) {
      persistTarget({ fps: preset.fps });
    }
  };

  const handleMove = (nodeId: string, origin: { x: number; y: number }) => {
    if (!draggingRef.current) {
      pushHistory();
    }
    const current = draftRef.current;
    const dragged = current.find((node) => node.id === nodeId);
    if (!dragged) {
      return;
    }
    const next = current.map((node) =>
      node.id === nodeId ? { ...node, origin: { x: origin.x, y: origin.y } } : node,
    );
    draftRef.current = next;
    setDraft(next);
    draftVersionRef.current += 1;
    setDirty(true);
  };

  const handleDragStart = () => {
    pushHistory();
    draggingRef.current = true;
    setDragging(true);
  };

  const handleDragEnd = () => {
    draggingRef.current = false;
    setDragging(false);
    persistDraft(draftRef.current, draftVersionRef.current);
  };

  const requestFit = () => setFitRequest((current) => current + 1);

  return {
    draft,
    visibleNodes,
    deviceGroups,
    deviceColorByDevice,
    selectedId,
    setSelectedId,
    selectedNode,
    selectedPortrait,
    dirty,
    saving,
    dragging,
    historyCount,
    fitRequest,
    requestFit,
    undo,
    setOrientation,
    setOrientationFor,
    target,
    resolutionValue,
    fpsValue,
    resolutionOptions,
    fpsOptions,
    selectResolution,
    selectFps,
    persistTarget,
    handleMove,
    handleDragStart,
    handleDragEnd,
  };
}

export type LayoutDraft = ReturnType<typeof useLayoutDraft>;
