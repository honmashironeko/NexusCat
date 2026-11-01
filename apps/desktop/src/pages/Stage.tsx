import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../api/client';
import {
  computeNeighbors,
  LAYOUT_DIRECTIONS,
  type LayoutDirection,
} from '../domain/layout';
import { activeNodes } from '../domain/group';
import { directionLabelKey } from '../domain/labels';
import { Icon } from '../components/Icon';
import { PermissionBanner } from '../components/PermissionBanner';
import { Toaster } from '../components/Toast';
import { useDoubleEsc } from '../hooks/useDoubleEsc';
import { useMediaBridge } from '../hooks/useMediaBridge';
import { useSession } from '../hooks/useSession';
import { useApp } from '../state/appStore';
import type { DecodedFrameInfo, PresentedWindowStats } from '../media/videoPipeline';
import { videoBackingSize } from '../media/videoCanvas';
import type { StageEdgeHintEvent } from '../api/types';

const DIRECTION_ICONS: Record<LayoutDirection, 'arrowLeft' | 'arrowRight' | 'arrowUp' | 'arrowDown'> = {
  left: 'arrowLeft',
  right: 'arrowRight',
  up: 'arrowUp',
  down: 'arrowDown',
};

const CURSOR_CANVAS_WIDTH = 18;
const CURSOR_CANVAS_HEIGHT = 22;
const CURSOR_TIP_OFFSET = 1;

function videoContext(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
  return canvas.getContext('2d', { alpha: false, desynchronized: true });
}

function paintSource(
  canvas: HTMLCanvasElement,
  source: CanvasImageSource,
  sourceWidth: number,
  sourceHeight: number,
): void {
  const cssWidth = canvas.clientWidth;
  const cssHeight = canvas.clientHeight;
  if (cssWidth <= 0 || cssHeight <= 0 || sourceWidth <= 0 || sourceHeight <= 0) {
    return;
  }
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const backing = videoBackingSize(
    cssWidth * dpr,
    cssHeight * dpr,
    sourceWidth,
    sourceHeight,
  );
  const pixelWidth = backing.width;
  const pixelHeight = backing.height;
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
  }
  const context = videoContext(canvas);
  if (!context) {
    return;
  }
  context.setTransform(1, 0, 0, 1, 0, 0);
  const scale = Math.min(pixelWidth / sourceWidth, pixelHeight / sourceHeight);
  const drawWidth = Math.max(1, Math.round(sourceWidth * scale));
  const drawHeight = Math.max(1, Math.round(sourceHeight * scale));
  const drawX = Math.round((pixelWidth - drawWidth) / 2);
  const drawY = Math.round((pixelHeight - drawHeight) / 2);
  if (drawX > 0 || drawY > 0 || drawWidth < pixelWidth || drawHeight < pixelHeight) {
    context.fillStyle = '#04070b';
    if (drawY > 0) {
      context.fillRect(0, 0, pixelWidth, drawY);
    }
    if (drawY + drawHeight < pixelHeight) {
      context.fillRect(0, drawY + drawHeight, pixelWidth, pixelHeight - drawY - drawHeight);
    }
    if (drawX > 0) {
      context.fillRect(0, drawY, drawX, drawHeight);
    }
    if (drawX + drawWidth < pixelWidth) {
      context.fillRect(drawX + drawWidth, drawY, pixelWidth - drawX - drawWidth, drawHeight);
    }
  }
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = 'low';
  context.drawImage(source, drawX, drawY, drawWidth, drawHeight);
}

interface ContentRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

function videoContentRect(
  canvas: HTMLCanvasElement,
  sourceWidth: number,
  sourceHeight: number,
): ContentRect | null {
  const bounds = canvas.getBoundingClientRect();
  if (
    bounds.width <= 0 ||
    bounds.height <= 0 ||
    sourceWidth <= 0 ||
    sourceHeight <= 0
  ) {
    return null;
  }
  const scale = Math.min(bounds.width / sourceWidth, bounds.height / sourceHeight);
  const width = sourceWidth * scale;
  const height = sourceHeight * scale;
  return {
    left: bounds.left + (bounds.width - width) / 2,
    top: bounds.top + (bounds.height - height) / 2,
    width,
    height,
  };
}

function paintProxyCursor(canvas: HTMLCanvasElement): void {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const width = Math.round(CURSOR_CANVAS_WIDTH * dpr);
  const height = Math.round(CURSOR_CANVAS_HEIGHT * dpr);
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  const context = canvas.getContext('2d');
  if (!context) {
    return;
  }
  context.setTransform(dpr, 0, 0, dpr, 0, 0);
  context.clearRect(0, 0, CURSOR_CANVAS_WIDTH, CURSOR_CANVAS_HEIGHT);
  const x = CURSOR_TIP_OFFSET;
  const y = CURSOR_TIP_OFFSET;
  context.beginPath();
  context.moveTo(x, y);
  context.lineTo(x + 0.6, y + 16.4);
  context.lineTo(x + 4.6, y + 12.6);
  context.lineTo(x + 7.2, y + 18.7);
  context.lineTo(x + 9.8, y + 17.5);
  context.lineTo(x + 7.3, y + 11.5);
  context.lineTo(x + 12.9, y + 11.4);
  context.closePath();
  context.fillStyle = '#ffffff';
  context.strokeStyle = 'rgba(0, 0, 0, 0.92)';
  context.lineWidth = 1.1;
  context.fill();
  context.stroke();
}

export function Stage({ active = true }: { active?: boolean }) {
  const { state, t, reportError } = useApp();
  const { session, currentNode, activeDeviceId } = useSession();
  const stageRef = useRef<HTMLDivElement>(null);
  const videoCanvasRef = useRef<HTMLCanvasElement>(null);
  const cursorCanvasRef = useRef<HTMLCanvasElement>(null);
  const videoRectRef = useRef<ContentRect | null>(null);
  const remoteCursorRef = useRef<{ x: number; y: number } | null>(null);
  const cursorFrameRef = useRef<number | null>(null);
  const pendingFrameRef = useRef<VideoFrame | null>(null);
  const paintFrameRef = useRef<number | null>(null);
  const presentedStatsRef = useRef({
    presented: 0,
    dropped: 0,
    paintMsTotal: 0,
    paintSamples: 0,
  });
  const exitingRef = useRef(false);
  const [switching, setSwitching] = useState(false);
  const [awaitingFrame, setAwaitingFrame] = useState(true);
  const [waitingVisible, setWaitingVisible] = useState(false);
  const [waitKey, setWaitKey] = useState(0);
  const [reconnectVisible, setReconnectVisible] = useState(false);
  const [reconnectBusy, setReconnectBusy] = useState(false);
  const [remotePaused, setRemotePaused] = useState(false);
  const [pauseShortcut, setPauseShortcut] = useState('');
  const [edgeHint, setEdgeHint] = useState<StageEdgeHintEvent | null>(null);
  const edgeHintTimerRef = useRef<number | null>(null);
  const inputOwnerRef = useRef(session?.input_owner);
  useEffect(() => {
    inputOwnerRef.current = session?.input_owner;
  }, [session?.input_owner]);
  const remotePausedRef = useRef(remotePaused);
  useEffect(() => {
    remotePausedRef.current = remotePaused;
  }, [remotePaused]);
  const lastLocalPointerAtRef = useRef(0);

  const drawRemoteCursor = useCallback(() => {
    if (cursorFrameRef.current !== null) {
      return;
    }
    cursorFrameRef.current = window.requestAnimationFrame(() => {
      cursorFrameRef.current = null;
      const canvas = cursorCanvasRef.current;
      const stage = stageRef.current;
      if (!canvas || !stage) {
        return;
      }
      if (inputOwnerRef.current === 'local' || remotePausedRef.current) {
        canvas.style.visibility = 'hidden';
        return;
      }
      const content = videoRectRef.current;
      const cursor = remoteCursorRef.current;
      if (!content || !cursor) {
        canvas.style.visibility = 'hidden';
        return;
      }
      const stageBounds = stage.getBoundingClientRect();
      const pointerX = content.left - stageBounds.left + cursor.x * content.width;
      const pointerY = content.top - stageBounds.top + cursor.y * content.height;
      canvas.style.visibility = 'visible';
      canvas.style.transform = `translate3d(${pointerX - CURSOR_TIP_OFFSET}px, ${
        pointerY - CURSOR_TIP_OFFSET
      }px, 0)`;
      paintProxyCursor(canvas);
    });
  }, []);

  const paintLatestFrame = useCallback(() => {
    paintFrameRef.current = null;
    const frame = pendingFrameRef.current;
    pendingFrameRef.current = null;
    if (!frame) {
      return;
    }
    const startedAt = performance.now();
    let painted = false;
    try {
      const canvas = videoCanvasRef.current;
      if (canvas) {
        paintSource(canvas, frame, frame.displayWidth, frame.displayHeight);
        videoRectRef.current = videoContentRect(
          canvas,
          frame.displayWidth,
          frame.displayHeight,
        );
        drawRemoteCursor();
        painted = true;
      } else {
        presentedStatsRef.current.dropped += 1;
      }
    } finally {
      frame.close();
      if (painted) {
        const stats = presentedStatsRef.current;
        stats.presented += 1;
        stats.paintMsTotal += performance.now() - startedAt;
        stats.paintSamples += 1;
      }
    }
  }, [drawRemoteCursor]);

  const takePresentedStats = useCallback((): PresentedWindowStats => {
    const stats = presentedStatsRef.current;
    const result = {
      presented: stats.presented,
      dropped: stats.dropped,
      present_ms: stats.paintSamples > 0 ? stats.paintMsTotal / stats.paintSamples : 0,
    };
    stats.presented = 0;
    stats.dropped = 0;
    stats.paintMsTotal = 0;
    stats.paintSamples = 0;
    return result;
  }, []);

  const handleVideoFrame = useCallback(
    (frame: VideoFrame, info: DecodedFrameInfo) => {
      if (!activeDeviceId || info.deviceId === activeDeviceId) {
        setAwaitingFrame(false);
      }
      let next: VideoFrame;
      try {
        next = frame.clone();
      } catch {
        return;
      }
      const previous = pendingFrameRef.current;
      pendingFrameRef.current = next;
      if (previous) {
        previous.close();
        presentedStatsRef.current.dropped += 1;
      }
      if (paintFrameRef.current === null) {
        paintFrameRef.current = window.requestAnimationFrame(paintLatestFrame);
      }
    },
    [activeDeviceId, paintLatestFrame],
  );

  const {
    bridgeStatus,
    videoProbe,
    streamEpoch,
    audioState,
    audioUnavailableReason,
    resumeAudio,
    requestActiveKeyframe,
  } = useMediaBridge({
    enabled: true,
    bridgeInfo: state.mediaBridge,
    activeDeviceId,
    audioEnabled: state.settings.audio_enabled,
    onVideoFrame: handleVideoFrame,
    takePresentedStats,
  });

  useEffect(() => {
    void resumeAudio();
  }, [resumeAudio]);

  const isMock = bridgeStatus === 'mock';
  const isControlling = Boolean(session && session.input_owner !== 'local');
  const neighbors = currentNode
    ? computeNeighbors(activeNodes(state.layout), currentNode.id)
    : null;
  const pairingOpen = Boolean(state.pairing);
  const h264Unavailable =
    !isMock && videoProbe !== null && (!videoProbe.supported || !videoProbe.h264);

  useEffect(
    () => () => {
      if (cursorFrameRef.current !== null) {
        window.cancelAnimationFrame(cursorFrameRef.current);
        cursorFrameRef.current = null;
      }
      if (paintFrameRef.current !== null) {
        window.cancelAnimationFrame(paintFrameRef.current);
        paintFrameRef.current = null;
      }
      pendingFrameRef.current?.close();
      pendingFrameRef.current = null;
    },
    [],
  );

  const handleHide = useCallback(async () => {
    if (exitingRef.current) {
      return;
    }
    exitingRef.current = true;
    try {
      await api.hideStage();
    } catch (error) {
      exitingRef.current = false;
      reportError(error, t('stage.error'));
    }
  }, [reportError, t]);

  const exitEnabled = !pairingOpen;
  const { armed } = useDoubleEsc(exitEnabled, () => {
    void handleHide();
  });

  useEffect(() => {
    if (!active || isMock) {
      return;
    }
    videoRectRef.current = null;
    if (paintFrameRef.current !== null) {
      window.cancelAnimationFrame(paintFrameRef.current);
      paintFrameRef.current = null;
    }
    pendingFrameRef.current?.close();
    pendingFrameRef.current = null;
    presentedStatsRef.current.presented = 0;
    presentedStatsRef.current.dropped = 0;
    presentedStatsRef.current.paintMsTotal = 0;
    presentedStatsRef.current.paintSamples = 0;
    setAwaitingFrame(true);
    setWaitingVisible(false);
    setWaitKey((value) => value + 1);
    requestActiveKeyframe();
  }, [active, activeDeviceId, currentNode?.id, streamEpoch, isMock, requestActiveKeyframe]);

  useEffect(() => {
    if (!active || !awaitingFrame) {
      setWaitingVisible(false);
      return;
    }
    const timer = window.setTimeout(() => setWaitingVisible(true), 120);
    return () => window.clearTimeout(timer);
  }, [active, awaitingFrame, waitKey]);

  useEffect(() => {
    if (!active || !awaitingFrame || isMock) {
      return;
    }
    const timer = window.setInterval(() => requestActiveKeyframe(), 900);
    return () => window.clearInterval(timer);
  }, [active, awaitingFrame, isMock, requestActiveKeyframe]);

  useEffect(() => {
    if (isMock) {
      return;
    }
    const handleResize = () => {
      drawRemoteCursor();
    };
    window.addEventListener('resize', handleResize);
    return () => {
      window.removeEventListener('resize', handleResize);
    };
  }, [isMock, drawRemoteCursor]);

  useEffect(() => {
    if (!exitEnabled) {
      return;
    }
    let armedEdge = false;
    let edgeTimer: ReturnType<typeof setTimeout> | null = null;
    const clearEdgeTimer = () => {
      if (edgeTimer !== null) {
        clearTimeout(edgeTimer);
        edgeTimer = null;
      }
    };
    const handlePointerMove = (event: PointerEvent) => {
      if (inputOwnerRef.current !== 'local') {
        clearEdgeTimer();
        return;
      }
      const width = window.innerWidth;
      const height = window.innerHeight;
      const insideHorizontalBand = event.clientX > 64 && event.clientX < width - 64;
      if (insideHorizontalBand) {
        armedEdge = true;
      }
      const inVerticalZone = event.clientY > 72 && event.clientY < height - 72;
      const atEdge =
        armedEdge &&
        inVerticalZone &&
        (event.clientX <= 3 || event.clientX >= width - 3);
      if (atEdge) {
        if (edgeTimer === null) {
          edgeTimer = setTimeout(() => {
            edgeTimer = null;
            if (armedEdge && inputOwnerRef.current === 'local') {
              armedEdge = false;
              void handleHide();
            }
          }, 100);
        }
      } else {
        clearEdgeTimer();
      }
    };
    window.addEventListener('pointermove', handlePointerMove, { passive: true });
    return () => {
      window.removeEventListener('pointermove', handlePointerMove);
      clearEdgeTimer();
    };
  }, [exitEnabled, handleHide]);

  useEffect(() => {
    const canvas = cursorCanvasRef.current;
    if (!canvas) {
      return;
    }
    if (!isControlling || remotePaused) {
      canvas.style.visibility = 'hidden';
      return;
    }
    drawRemoteCursor();
  }, [isControlling, remotePaused, drawRemoteCursor]);

  useEffect(() => {
    remoteCursorRef.current = null;
    videoRectRef.current = null;
    lastLocalPointerAtRef.current = 0;
    drawRemoteCursor();
  }, [activeDeviceId, drawRemoteCursor]);

  useEffect(() => {
    if (isMock) {
      return;
    }
    const handlePointerMove = (event: PointerEvent) => {
      if (inputOwnerRef.current === 'local' || remotePausedRef.current) {
        return;
      }
      const content = videoRectRef.current;
      if (!content || content.width <= 0 || content.height <= 0) {
        return;
      }
      const x = Math.min(
        1,
        Math.max(0, (event.clientX - content.left) / content.width),
      );
      const y = Math.min(
        1,
        Math.max(0, (event.clientY - content.top) / content.height),
      );
      remoteCursorRef.current = { x, y };
      lastLocalPointerAtRef.current = performance.now();
      drawRemoteCursor();
    };
    window.addEventListener('pointermove', handlePointerMove, { passive: true });
    return () => {
      window.removeEventListener('pointermove', handlePointerMove);
    };
  }, [isMock, drawRemoteCursor]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void api
      .subscribe('stage-switching', (payload) => {
        if (!cancelled) {
          setSwitching(payload.active);
        }
      })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void api
      .subscribe('remote-cursor', (payload) => {
        if (cancelled) {
          return;
        }
        if (activeDeviceId && payload.device_id !== activeDeviceId) {
          return;
        }
        if (performance.now() - lastLocalPointerAtRef.current < 250) {
          return;
        }
        remoteCursorRef.current = { x: payload.x, y: payload.y };
        drawRemoteCursor();
      })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [activeDeviceId, drawRemoteCursor]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void api
      .subscribe('remote-pause', (payload) => {
        if (cancelled) {
          return;
        }
        setRemotePaused(payload.paused);
        setPauseShortcut(payload.shortcut);
      })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void api
      .subscribe('stage-edge-hint', (payload) => {
        if (cancelled) {
          return;
        }
        setEdgeHint(payload);
        if (edgeHintTimerRef.current !== null) {
          window.clearTimeout(edgeHintTimerRef.current);
        }
        edgeHintTimerRef.current = window.setTimeout(() => {
          edgeHintTimerRef.current = null;
          setEdgeHint(null);
        }, 1600);
      })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
      if (edgeHintTimerRef.current !== null) {
        window.clearTimeout(edgeHintTimerRef.current);
        edgeHintTimerRef.current = null;
      }
    };
  }, []);

  const resolveOverlay = (): { icon: 'warning'; title: string; body?: string } | null => {
    if (isMock) {
      return null;
    }
    if (h264Unavailable) {
      return {
        icon: 'warning',
        title: t('stage.webcodecsUnavailable'),
        body: t('stage.webcodecsHelp'),
      };
    }
    if (bridgeStatus === 'error' || bridgeStatus === 'closed') {
      return {
        icon: 'warning',
        title: t('stage.noSignal'),
      };
    }
    return null;
  };
  const overlay = resolveOverlay();

  const stageDirectionClass: Record<LayoutDirection, string> = {
    left: 'stage-edge-left',
    right: 'stage-edge-right',
    up: 'stage-edge-up',
    down: 'stage-edge-down',
  };

  const waitingForFrame = active && awaitingFrame && waitingVisible;

  const retryConnection = async () => {
    if (reconnectBusy) {
      return;
    }
    setReconnectBusy(true);
    try {
      await api.retryConnection();
    } catch (error) {
      reportError(error, t('stage.error'));
    } finally {
      setReconnectBusy(false);
    }
  };

  useEffect(() => {
    if (!waitingForFrame) {
      setReconnectVisible(false);
      return;
    }
    const timer = window.setTimeout(() => setReconnectVisible(true), 2500);
    return () => window.clearTimeout(timer);
  }, [waitingForFrame, waitKey]);

  const rootClassName = [
    'stage-root',
    isControlling ? 'is-controlling' : '',
    remotePaused ? 'is-remote-paused' : '',
    waitingForFrame ? 'is-awaiting-frame' : '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <div
      ref={stageRef}
      className={rootClassName}
      aria-label={t('stage.title')}
      onPointerDown={() => {
        void resumeAudio();
      }}
    >
      {currentNode ? (
        <div className={isControlling ? 'stage-identity is-controlling' : 'stage-identity'}>
          <Icon name="monitor" size={12} />
          <span>{currentNode.device_name}</span>
          <span className="stage-identity-display">
            {currentNode.display_name.trim() || t('layout.monitorIndex', {
              index: currentNode.monitor_index + 1,
            })}
          </span>
        </div>
      ) : null}
      <div className="stage-viewport">
        {isMock ? (
          <div className="stage-mock-desktop" aria-hidden="true">
            <div className="mock-menu-bar">
              <span className="mock-brand-dot" />
              <span>NexusCat</span>
              <span>File</span>
              <span>View</span>
              <span className="mock-time">14:32</span>
            </div>
            <div className="mock-desktop-body">
              <div className="mock-window">
                <div className="mock-window-title">
                  <span className="mock-window-dots">
                    <i />
                    <i />
                    <i />
                  </span>
                  <span>NexusCat</span>
                </div>
                <div className="mock-window-content">
                  <div className="mock-sidebar">
                    <span />
                    <span />
                    <span />
                  </div>
                  <div className="mock-canvas">
                    <span className="mock-line wide" />
                    <span className="mock-line medium" />
                    <span className="mock-block" />
                    <div className="mock-tiles">
                      <span />
                      <span />
                    </div>
                  </div>
                </div>
              </div>
              <div className="mock-dock">
                <span />
                <span />
                <span />
                <span />
              </div>
            </div>
          </div>
        ) : null}

        <canvas
          ref={videoCanvasRef}
          className={isMock ? 'stage-video is-mock' : 'stage-video'}
          role="img"
          aria-label={t('stage.videoAria')}
        />

        {!isControlling && neighbors
          ? LAYOUT_DIRECTIONS.map((direction) => {
              const neighbor = neighbors[direction];
              const sameDevice =
                Boolean(neighbor && currentNode) &&
                neighbor?.device_id === currentNode?.device_id;
              const displayName = neighbor
                ? neighbor.display_name.trim() || t('layout.monitorIndex', {
                    index: neighbor.monitor_index + 1,
                  })
                : '';
              const neighborName = neighbor
                ? `${neighbor.device_name} · ${displayName}`
                : '';
              return (
                <div
                  key={direction}
                  className={`stage-neighbor ${stageDirectionClass[direction]} ${
                    neighbor ? '' : 'is-edge'
                  } ${sameDevice ? 'is-same-device' : ''}`}
                  aria-label={
                    neighbor
                      ? t(
                          sameDevice
                            ? 'stage.sameDeviceNeighbor'
                            : 'stage.neighborDirection',
                          {
                            direction: t(directionLabelKey(direction)),
                            name: sameDevice ? displayName : neighborName,
                          },
                        )
                      : t('stage.edgeDirection', {
                          direction: t(directionLabelKey(direction)),
                        })
                  }
                >
                  <Icon name={DIRECTION_ICONS[direction]} size={12} />
                  <span>
                    {neighbor
                      ? sameDevice
                        ? t('stage.sameDeviceNeighbor', {
                            name: displayName,
                          })
                        : neighborName
                      : t('layout.noNeighbor')}
                  </span>
                </div>
              );
            })
          : null}
      </div>

      <div className="stage-permission-layer">
        <PermissionBanner />
      </div>

      {isMock && !isControlling ? (
        <div className="stage-mode-chip">
          <Icon name="info" size={12} />
          {t('stage.mockHint')}
        </div>
      ) : null}

      {remotePaused ? (
        <div className="stage-paused-chip" role="status">
          <Icon name="pause" size={12} />
          {t('stage.paused', {
            shortcut: pauseShortcut.trim() || t('settings.shortcutUnset'),
          })}
        </div>
      ) : null}

      {edgeHint ? (
        <div
          className={`stage-edge-hint is-${edgeHint.direction}`}
          role="status"
        >
          <Icon name={DIRECTION_ICONS[edgeHint.direction]} size={13} />
          <span>
            {t('stage.edgeNoDevice', {
              direction: t(directionLabelKey(edgeHint.direction)),
            })}
          </span>
        </div>
      ) : null}

      {audioUnavailableReason ? (
        <div className="stage-audio-notice" role="status" title={audioUnavailableReason}>
          <Icon name="volume" size={13} />
          <span>{t('stage.audioFallbackHint')}</span>
        </div>
      ) : null}

      {audioState === 'suspended' && state.settings.audio_enabled && !audioUnavailableReason ? (
        <button
          type="button"
          className="stage-audio-prompt"
          onClick={() => void resumeAudio()}
        >
          <Icon name="volume" size={13} />
          {t('stage.audioEnable')}
        </button>
      ) : null}

      {active && (switching || waitingForFrame) ? (
        <div className="stage-overlay" role="status">
          <div className="stage-overlay-card">
            <span className="stage-overlay-icon is-waiting" aria-hidden="true">
              <Icon name="refresh" size={24} />
            </span>
            <strong>{switching ? t('stage.switching') : t('stage.connecting')}</strong>
            <span>
              {switching ? t('stage.switchingHint') : t('stage.connectingHint')}
            </span>
            {!switching && reconnectVisible ? (
              <button
                type="button"
                className="btn compact stage-retry"
                disabled={reconnectBusy}
                onClick={() => void retryConnection()}
              >
                {reconnectBusy ? (
                  <span className="spinner" aria-hidden="true" />
                ) : (
                  <Icon name="refresh" size={12} />
                )}
                {reconnectBusy ? t('common.busy') : t('stage.reconnect')}
              </button>
            ) : null}
          </div>
        </div>
      ) : active && overlay ? (
        <div className="stage-overlay" role="status">
          <div className="stage-overlay-card">
            <span className="stage-overlay-icon" aria-hidden="true">
              <Icon name={overlay.icon} size={24} />
            </span>
            <strong>{overlay.title}</strong>
            {overlay.body ? <span>{overlay.body}</span> : null}
            {h264Unavailable && videoProbe?.reason ? (
              <code className="stage-error-detail">{videoProbe.reason}</code>
            ) : null}
            {session?.last_error ? (
              <span className="stage-error-detail">{session.last_error}</span>
            ) : null}
          </div>
        </div>
      ) : null}

      <footer className={isControlling ? 'stage-footer is-controlling' : 'stage-footer'}>
        <span className="stage-offline-note">
          {!isControlling && session?.last_error && !overlay ? session.last_error : ''}
        </span>
        <div className={armed ? 'stage-exit-hint is-armed' : 'stage-exit-hint'}>
          <kbd>Esc</kbd>
          <span>
            {armed
              ? t('stage.escAgain')
              : t('stage.exitHint', {
                  shortcut:
                    state.settings.shortcuts.toggle_stage.trim() ||
                    t('settings.shortcutUnset'),
                })}
          </span>
        </div>
        <span className="stage-footer-spacer" />
      </footer>

      <canvas ref={cursorCanvasRef} className="stage-cursor" aria-hidden="true" />
      <Toaster />
    </div>
  );
}
