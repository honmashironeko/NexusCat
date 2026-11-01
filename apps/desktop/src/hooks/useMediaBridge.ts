import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../api/client';
import type { MediaBridgeInfo } from '../api/types';
import { isControlMessageType } from '../media/binary';
import {
  MediaBridgeClient,
  type MediaBridgeStatus,
} from '../media/bridge';
import {
  RemoteAudioPlayer,
  type AudioPlayerState,
} from '../media/audioPipeline';
import {
  probeH264Support,
  RemoteVideoPipeline,
  type DecodedFrameInfo,
  type PresentedWindowStats,
  type VideoProbeResult,
} from '../media/videoPipeline';

export interface UseMediaBridgeOptions {
  enabled: boolean;
  bridgeInfo: MediaBridgeInfo | null;
  activeDeviceId: string | null;
  audioEnabled: boolean;
  onVideoFrame: (frame: VideoFrame, info: DecodedFrameInfo) => void;
  takePresentedStats?: () => PresentedWindowStats;
}

export interface UseMediaBridgeResult {
  bridgeStatus: MediaBridgeStatus;
  videoProbe: VideoProbeResult | null;
  pipeline: RemoteVideoPipeline | null;
  streamEpoch: number;
  audioState: AudioPlayerState;
  audioUnavailableReason: string | null;
  resumeAudio: () => Promise<void>;
  requestActiveKeyframe: () => void;
}

export function useMediaBridge({
  enabled,
  bridgeInfo,
  activeDeviceId,
  audioEnabled,
  onVideoFrame,
  takePresentedStats,
}: UseMediaBridgeOptions): UseMediaBridgeResult {
  const [bridgeStatus, setBridgeStatus] = useState<MediaBridgeStatus>('idle');
  const [videoProbe, setVideoProbe] = useState<VideoProbeResult | null>(null);
  const [pipeline, setPipeline] = useState<RemoteVideoPipeline | null>(null);
  const [audioState, setAudioState] = useState<AudioPlayerState>('idle');
  const [audioUnavailableReason, setAudioUnavailableReason] = useState<string | null>(null);
  const [streamEpoch, setStreamEpoch] = useState(0);

  const onVideoFrameRef = useRef(onVideoFrame);
  onVideoFrameRef.current = onVideoFrame;
  const takePresentedStatsRef = useRef(takePresentedStats);
  takePresentedStatsRef.current = takePresentedStats;
  const pipelineRef = useRef<RemoteVideoPipeline | null>(null);
  const audioPlayerRef = useRef<RemoteAudioPlayer | null>(null);
  const audioEnabledRef = useRef(audioEnabled);
  audioEnabledRef.current = audioEnabled;
  const requestTimesRef = useRef(new Map<number, number>());

  const requestKeyframe = useCallback((streamId: number) => {
    const now = Date.now();
    const previous = requestTimesRef.current.get(streamId) ?? 0;
    if (now - previous < 750) {
      return;
    }
    requestTimesRef.current.set(streamId, now);
    api.requestKeyframe(streamId).catch(() => undefined);
  }, []);

  useEffect(() => {
    if (!enabled) {
      setVideoProbe(null);
      setPipeline(null);
      pipelineRef.current?.dispose();
      pipelineRef.current = null;
      return;
    }
    let cancelled = false;
    void probeH264Support().then((result) => {
      if (cancelled) {
        return;
      }
      setVideoProbe(result);
      api
        .reportWebCodecsProbe({
          supported: result.supported,
          h264: result.h264,
          reason: result.reason,
        })
        .catch(() => undefined);
      if (!result.h264 || !result.codec) {
        setPipeline(null);
        return;
      }
      const nextPipeline = new RemoteVideoPipeline({
        codec: result.codec,
        acceleration: result.acceleration ?? 'no-preference',
        onFrame: (frame, info) => {
          onVideoFrameRef.current(frame, info);
        },
        takePresentedStats: () =>
          takePresentedStatsRef.current?.() ?? {
            presented: 0,
            dropped: 0,
            present_ms: 0,
          },
        onPresentStats: (stats) => {
          api.reportPresentStats(stats).catch(() => undefined);
        },
        onDecoderError: (_streamId, _error) => {
          requestKeyframe(_streamId);
        },
        onRequestKeyframe: (streamId) => {
          requestKeyframe(streamId);
        },
      });
      pipelineRef.current = nextPipeline;
      setPipeline(nextPipeline);
    });
    return () => {
      cancelled = true;
      pipelineRef.current?.dispose();
      pipelineRef.current = null;
      setPipeline(null);
    };
  }, [enabled, requestKeyframe]);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    const player = new RemoteAudioPlayer({
      onUnavailable: (reason) => {
        setAudioUnavailableReason(reason);
      },
      onStateChange: (state) => {
        setAudioState(state);
      },
    });
    player.setEnabled(audioEnabledRef.current);
    audioPlayerRef.current = player;
    return () => {
      player.dispose();
      if (audioPlayerRef.current === player) {
        audioPlayerRef.current = null;
      }
    };
  }, [enabled]);

  useEffect(() => {
    audioPlayerRef.current?.setEnabled(audioEnabled);
    if (!audioEnabled) {
      setAudioState('idle');
    }
  }, [audioEnabled]);

  useEffect(() => {
    if (!enabled || !bridgeInfo) {
      setBridgeStatus('idle');
      return;
    }
    if (!bridgeInfo.port) {
      setBridgeStatus('mock');
      return;
    }
    const client = new MediaBridgeClient({
      onFrame: (frame) => {
        if (frame.kind === 'video') {
          pipelineRef.current?.handleVideoFrame(frame);
          return;
        }
        if (frame.kind === 'audio') {
          if (audioEnabledRef.current) {
            audioPlayerRef.current?.handleFrame(frame);
          }
          return;
        }
        if (frame.kind === 'control') {
          if (
            isControlMessageType(frame.message, 'stream_stopped') &&
            typeof frame.message.stream_id === 'number'
          ) {
            pipelineRef.current?.streamStopped(frame.message.stream_id);
            return;
          }
          if (
            isControlMessageType(frame.message, 'stream_started') &&
            typeof frame.message.stream_id === 'number'
          ) {
            const streamId = frame.message.stream_id;
            pipelineRef.current?.streamStopped(streamId);
            requestTimesRef.current.delete(streamId);
            setStreamEpoch((value) => value + 1);
            requestKeyframe(streamId);
          }
        }
      },
      onStatus: (status) => {
        setBridgeStatus(status);
      },
    });
    client.connect(bridgeInfo);
    return () => {
      client.close();
    };
  }, [enabled, bridgeInfo?.port, bridgeInfo?.token, bridgeInfo]);

  useEffect(() => {
    const activePipeline = pipelineRef.current;
    if (!activePipeline) {
      return;
    }
    activePipeline.setActiveDevice(activeDeviceId);
    if (!activeDeviceId) {
      return;
    }
    const streamId = activePipeline.getStreamIdForDevice(activeDeviceId) ?? 1;
    if (bridgeStatus === 'connected' || bridgeStatus === 'reconnecting') {
      requestKeyframe(streamId);
    }
  }, [pipeline, activeDeviceId, bridgeStatus, requestKeyframe]);

  const resumeAudio = useCallback(async () => {
    await audioPlayerRef.current?.resume();
  }, []);

  const requestActiveKeyframe = useCallback(() => {
    const activePipeline = pipelineRef.current;
    if (!activePipeline) {
      return;
    }
    const deviceId = activePipeline.getActiveDevice();
    if (!deviceId) {
      return;
    }
    const streamId = activePipeline.getStreamIdForDevice(deviceId) ?? 1;
    requestKeyframe(streamId);
  }, [requestKeyframe]);

  return {
    bridgeStatus,
    videoProbe,
    pipeline,
    streamEpoch,
    audioState,
    audioUnavailableReason,
    resumeAudio,
    requestActiveKeyframe,
  };
}
