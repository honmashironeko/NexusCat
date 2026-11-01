import type { VideoMediaFrame } from './binary';

export const H264_CODEC_CANDIDATES = [
  'avc1.42E034',
  'avc1.42E033',
  'avc1.42E02A',
  'avc1.4D4034',
  'avc1.640034',
  'avc1.4D401F',
  'avc1.42E01E',
] as const;

export type DecoderAcceleration = 'prefer-hardware' | 'no-preference';

const DECODER_ACCELERATION_CANDIDATES: DecoderAcceleration[] = [
  'prefer-hardware',
  'no-preference',
];

export interface VideoProbeResult {
  supported: boolean;
  h264: boolean;
  codec: string | null;
  acceleration: DecoderAcceleration | null;
  reason: string | null;
}

export interface DecodedFrameInfo {
  deviceId: string;
  streamId: number;
  width: number;
  height: number;
  timestampUs: number;
}

export interface PresentStats {
  decode_ms: number;
  present_ms: number;
  frames: number;
  presented_frames: number;
  dropped_frames: number;
  timestamp_us: number;
}

export interface PresentedWindowStats {
  presented: number;
  dropped: number;
  present_ms: number;
}

export interface VideoPipelineOptions {
  codec: string;
  acceleration: DecoderAcceleration;
  onFrame: (frame: VideoFrame, info: DecodedFrameInfo) => void;
  onPresentStats?: (stats: PresentStats) => void;
  takePresentedStats?: () => PresentedWindowStats;
  onDecoderError?: (streamId: number, error: unknown) => void;
  onRequestKeyframe?: (streamId: number) => void;
}

interface DecoderEntry {
  key: string;
  deviceId: string;
  streamId: number;
  decoder: VideoDecoder;
  waitingForKeyframe: boolean;
  configuredWidth: number;
  configuredHeight: number;
  pendingStarts: Map<number, number>;
  closed: boolean;
}

function hasVideoDecoder(): boolean {
  return typeof VideoDecoder !== 'undefined';
}

let probeCache: Promise<VideoProbeResult> | null = null;

export function probeH264Support(): Promise<VideoProbeResult> {
  probeCache ??= detectH264Support();
  return probeCache;
}

export function resetH264ProbeCache(): void {
  probeCache = null;
}

async function detectH264Support(): Promise<VideoProbeResult> {
  if (!hasVideoDecoder()) {
    return {
      supported: false,
      h264: false,
      codec: null,
      acceleration: null,
      reason: 'WebCodecs VideoDecoder is unavailable',
    };
  }
  const failures: string[] = [];
  for (const acceleration of DECODER_ACCELERATION_CANDIDATES) {
    for (const codec of H264_CODEC_CANDIDATES) {
      try {
        const support = await VideoDecoder.isConfigSupported({
          codec,
          codedWidth: 1920,
          codedHeight: 1080,
          optimizeForLatency: true,
          hardwareAcceleration: acceleration,
        });
        if (support.supported) {
          return {
            supported: true,
            h264: true,
            codec,
            acceleration,
            reason: null,
          };
        }
        failures.push(`${codec} (${acceleration}): unsupported`);
      } catch (error) {
        failures.push(
          `${codec} (${acceleration}): ${error instanceof Error ? error.message : String(error)}`,
        );
      }
    }
  }
  return {
    supported: true,
    h264: false,
    codec: null,
    acceleration: null,
    reason:
      failures.length > 0
        ? `No supported H.264 profile (${failures.join('; ')})`
        : 'No supported H.264 profile',
  };
}

export class RemoteVideoPipeline {
  private readonly codec: string;
  private acceleration: DecoderAcceleration;
  private readonly options: VideoPipelineOptions;
  private readonly decoders = new Map<string, DecoderEntry>();
  private activeDeviceId: string | null = null;
  private decodedFrames = 0;
  private droppedFrames = 0;
  private decodeTimeTotal = 0;
  private presentTimeTotal = 0;
  private lastStatsReportAt = 0;
  private disposed = false;

  constructor(options: VideoPipelineOptions) {
    this.codec = options.codec;
    this.acceleration = options.acceleration;
    this.options = options;
  }

  setActiveDevice(deviceId: string | null): void {
    this.activeDeviceId = deviceId;
    this.decodedFrames = 0;
    this.droppedFrames = 0;
    this.decodeTimeTotal = 0;
    this.presentTimeTotal = 0;
  }

  getActiveDevice(): string | null {
    return this.activeDeviceId;
  }

  getStreamIdForDevice(deviceId: string): number | null {
    for (const entry of this.decoders.values()) {
      if (entry.deviceId === deviceId) {
        return entry.streamId;
      }
    }
    return null;
  }

  handleVideoFrame(frame: VideoMediaFrame): void {
    if (this.disposed || !this.codec) {
      return;
    }
    const key = `${frame.deviceId}:${frame.streamId}`;
    let entry = this.decoders.get(key);
    const dimensionsChanged =
      entry !== undefined &&
      (entry.configuredWidth !== frame.width || entry.configuredHeight !== frame.height);

    if (frame.keyframe) {
      if (!entry || entry.closed || dimensionsChanged) {
        if (entry) {
          this.closeEntry(entry);
        }
        const created = this.createEntry(frame, key);
        if (!created) {
          this.droppedFrames += 1;
          return;
        }
        this.decoders.set(key, created);
        created.waitingForKeyframe = false;
        entry = created;
      } else {
        entry.waitingForKeyframe = false;
      }
    } else if (!entry || entry.waitingForKeyframe || entry.closed) {
      this.droppedFrames += 1;
      return;
    }

    if (!frame.keyframe && entry.decoder.decodeQueueSize > 8) {
      this.droppedFrames += 1;
      return;
    }

    try {
      entry.pendingStarts.set(frame.timestampUs, performance.now());
      if (entry.pendingStarts.size > 24) {
        const oldestTimestamp = entry.pendingStarts.keys().next().value;
        if (oldestTimestamp !== undefined) {
          entry.pendingStarts.delete(oldestTimestamp);
        }
      }
      const chunk = new EncodedVideoChunk({
        type: frame.keyframe ? 'key' : 'delta',
        timestamp: frame.timestampUs,
        data: frame.payload,
      });
      entry.decoder.decode(chunk);
    } catch (error) {
      this.droppedFrames += 1;
      this.closeEntry(entry);
      this.decoders.delete(key);
      this.options.onDecoderError?.(frame.streamId, error);
      this.options.onRequestKeyframe?.(frame.streamId);
    }
  }

  streamStopped(streamId: number): void {
    for (const [key, entry] of this.decoders) {
      if (entry.streamId === streamId) {
        this.closeEntry(entry);
        this.decoders.delete(key);
      }
    }
  }

  dispose(): void {
    this.disposed = true;
    for (const entry of this.decoders.values()) {
      this.closeEntry(entry);
    }
    this.decoders.clear();
  }

  private createEntry(frame: VideoMediaFrame, key: string): DecoderEntry | null {
    const primary = this.buildDecoder(frame, key, this.acceleration);
    if (primary) {
      return primary;
    }
    if (this.acceleration === 'prefer-hardware') {
      const fallback = this.buildDecoder(frame, key, 'no-preference');
      if (fallback) {
        this.acceleration = 'no-preference';
        return fallback;
      }
    }
    this.options.onDecoderError?.(
      frame.streamId,
      new Error('No supported H.264 decoder configuration'),
    );
    this.options.onRequestKeyframe?.(frame.streamId);
    return null;
  }

  private buildDecoder(
    frame: VideoMediaFrame,
    key: string,
    acceleration: DecoderAcceleration,
  ): DecoderEntry | null {
    const pendingStarts = new Map<number, number>();
    let entry: DecoderEntry;
    let decoder: VideoDecoder;
    try {
      decoder = new VideoDecoder({
        output: (decoded) => {
          this.handleDecodedFrame(entry, decoded);
        },
        error: (error) => {
          if (entry.closed) {
            return;
          }
          entry.closed = true;
          this.decoders.delete(key);
          if (this.acceleration === 'prefer-hardware') {
            this.acceleration = 'no-preference';
          }
          this.options.onDecoderError?.(frame.streamId, error);
          this.options.onRequestKeyframe?.(frame.streamId);
          try {
            decoder.close();
          } catch {
          }
        },
      });
    } catch {
      return null;
    }
    entry = {
      key,
      deviceId: frame.deviceId,
      streamId: frame.streamId,
      decoder,
      waitingForKeyframe: true,
      configuredWidth: frame.width,
      configuredHeight: frame.height,
      pendingStarts,
      closed: false,
    };
    try {
      decoder.configure({
        codec: this.codec,
        codedWidth: frame.width,
        codedHeight: frame.height,
        optimizeForLatency: true,
        hardwareAcceleration: acceleration,
      });
      return entry;
    } catch {
      entry.closed = true;
      try {
        decoder.close();
      } catch {
      }
      return null;
    }
  }

  private handleDecodedFrame(entry: DecoderEntry, frame: VideoFrame): void {
    if (entry.closed || this.disposed) {
      frame.close();
      return;
    }
    const outputAt = performance.now();
    const submittedAt = entry.pendingStarts.get(frame.timestamp);
    entry.pendingStarts.delete(frame.timestamp);
    try {
      if (entry.deviceId === this.activeDeviceId) {
        this.options.onFrame(frame, {
          deviceId: entry.deviceId,
          streamId: entry.streamId,
          width: frame.displayWidth,
          height: frame.displayHeight,
          timestampUs: frame.timestamp,
        });
        const presentAt = performance.now();
        const decodeMs =
          submittedAt === undefined ? 0 : Math.max(0, outputAt - submittedAt);
        const presentMs = Math.max(0, presentAt - outputAt);
        this.decodedFrames += 1;
        this.recordStats(
          decodeMs,
          presentMs,
          frame.timestamp,
          presentAt,
          this.decodedFrames,
        );
      }
    } finally {
      frame.close();
    }
  }

  private recordStats(
    decodeMs: number,
    presentMs: number,
    frameTimestampUs: number,
    at: number,
    frames: number,
  ): void {
    if (!this.options.onPresentStats) {
      return;
    }
    this.decodeTimeTotal += decodeMs;
    this.presentTimeTotal += presentMs;
    if (this.lastStatsReportAt === 0) {
      this.lastStatsReportAt = at;
      return;
    }
    if (at - this.lastStatsReportAt < 1000) {
      return;
    }
    const count = Math.max(1, this.decodedFrames);
    const presented = this.options.takePresentedStats?.();
    const pipelineDropped = this.droppedFrames;
    this.droppedFrames = 0;
    this.options.onPresentStats({
      decode_ms: this.decodeTimeTotal / count,
      present_ms: presented ? presented.present_ms : this.presentTimeTotal / count,
      frames,
      presented_frames: presented ? presented.presented : frames,
      dropped_frames: pipelineDropped + (presented?.dropped ?? 0),
      timestamp_us: frameTimestampUs,
    });
    this.decodedFrames = 0;
    this.decodeTimeTotal = 0;
    this.presentTimeTotal = 0;
    this.lastStatsReportAt = at;
  }

  private closeEntry(entry: DecoderEntry): void {
    if (entry.closed) {
      return;
    }
    entry.closed = true;
    entry.pendingStarts.clear();
    try {
      entry.decoder.close();
    } catch {
    }
  }
}
