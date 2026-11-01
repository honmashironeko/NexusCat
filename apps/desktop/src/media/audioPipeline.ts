
import type { AudioMediaFrame } from './binary';

export type AudioPlayerState = 'idle' | 'ready' | 'suspended' | 'unavailable';

export interface AudioPlayerCallbacks {
  onUnavailable?: (reason: string) => void;
  onStateChange?: (state: AudioPlayerState) => void;
}

type OpusStatus = 'idle' | 'preparing' | 'ready' | 'unavailable';

function buildOpusHead(channels: number, sampleRate: number): Uint8Array {
  const bytes = new Uint8Array(19);
  bytes.set([0x4f, 0x70, 0x75, 0x73, 0x48, 0x65, 0x61, 0x64], 0);
  const view = new DataView(bytes.buffer);
  view.setUint8(8, 1);
  view.setUint8(9, Math.min(2, Math.max(1, channels)));
  view.setUint16(10, 312, true);
  view.setUint32(12, sampleRate, true);
  view.setInt16(16, 0, true);
  view.setUint8(18, 0);
  return bytes;
}

export class RemoteAudioPlayer {
  private readonly callbacks: AudioPlayerCallbacks;
  private context: AudioContext | null = null;
  private gain: GainNode | null = null;
  private nextStartTime = 0;
  private enabled = true;
  private disposed = false;
  private unavailableNotified = false;
  private opusStatus: OpusStatus = 'idle';
  private opusKey = '';
  private opusDecoder: AudioDecoder | null = null;
  private opusQueue: AudioMediaFrame[] = [];
  private state: AudioPlayerState = 'idle';

  constructor(callbacks: AudioPlayerCallbacks = {}) {
    this.callbacks = callbacks;
  }

  getState(): AudioPlayerState {
    return this.state;
  }

  setEnabled(enabled: boolean): void {
    this.enabled = enabled;
    if (this.gain) {
      this.gain.gain.value = enabled ? 1 : 0;
    }
    if (enabled) {
      this.nextStartTime = 0;
    }
  }

  async resume(): Promise<void> {
    if (this.disposed) {
      return;
    }
    const context = this.ensureContext();
    if (!context) {
      return;
    }
    if (context.state === 'suspended') {
      try {
        await context.resume();
      } catch {
        this.updateState('suspended');
        return;
      }
    }
    this.updateState(context.state === 'running' ? 'ready' : 'suspended');
  }

  handleFrame(frame: AudioMediaFrame): void {
    if (this.disposed || !this.enabled || frame.payload.byteLength === 0) {
      return;
    }
    if (frame.codec === 'pcm-s16le') {
      this.handlePcm(frame);
      return;
    }
    if (frame.codec === 'opus') {
      this.handleOpus(frame);
    }
  }

  dispose(): void {
    this.disposed = true;
    this.opusQueue = [];
    this.opusStatus = 'unavailable';
    if (this.opusDecoder) {
      try {
        this.opusDecoder.close();
      } catch {
      }
      this.opusDecoder = null;
    }
    if (this.context) {
      void this.context.close().catch(() => undefined);
      this.context = null;
      this.gain = null;
    }
    this.updateState('idle');
  }

  private ensureContext(): AudioContext | null {
    if (this.disposed) {
      return null;
    }
    if (!this.context || this.context.state === 'closed') {
      if (typeof AudioContext === 'undefined') {
        this.markUnavailable('WebAudio AudioContext is unavailable');
        return null;
      }
      try {
        this.context = new AudioContext({ latencyHint: 'interactive' });
        this.gain = this.context.createGain();
        this.gain.gain.value = this.enabled ? 1 : 0;
        this.gain.connect(this.context.destination);
        this.updateState(
          this.context.state === 'running' ? 'ready' : 'suspended',
        );
      } catch (error) {
        this.markUnavailable(error instanceof Error ? error.message : String(error));
        return null;
      }
    }
    return this.context;
  }

  private handlePcm(frame: AudioMediaFrame): void {
    const channels = Math.min(8, Math.max(1, frame.channels));
    const sampleRate = Math.max(8000, Math.min(192000, frame.sampleRate || 48000));
    const bytesPerFrame = channels * 2;
    const sampleCount = Math.floor(frame.payload.byteLength / bytesPerFrame);
    if (sampleCount <= 0) {
      return;
    }
    const context = this.ensureContext();
    if (!context) {
      return;
    }
    let buffer: AudioBuffer;
    try {
      buffer = context.createBuffer(channels, sampleCount, sampleRate);
    } catch (error) {
      this.markUnavailable(error instanceof Error ? error.message : String(error));
      return;
    }
    const payload = frame.payload;
    for (let channel = 0; channel < channels; channel += 1) {
      const target = buffer.getChannelData(channel);
      for (let index = 0; index < sampleCount; index += 1) {
        const offset = (index * channels + channel) * 2;
        const low = payload[offset] ?? 0;
        const high = payload[offset + 1] ?? 0;
        const value = (low | (high << 8)) << 16 >> 16;
        target[index] = value / 32768;
      }
    }
    this.playBuffer(buffer, context);
  }

  private playBuffer(buffer: AudioBuffer, context: AudioContext): void {
    if (!this.gain || this.disposed) {
      return;
    }
    if (context.state !== 'running') {
      this.updateState('suspended');
      return;
    }
    const now = context.currentTime;
    let startAt = Math.max(now + 0.02, this.nextStartTime);
    if (startAt - now > 0.4) {
      startAt = now + 0.02;
    }
    try {
      const source = context.createBufferSource();
      source.buffer = buffer;
      source.connect(this.gain);
      source.start(startAt);
    } catch (error) {
      this.markUnavailable(error instanceof Error ? error.message : String(error));
      return;
    }
    this.nextStartTime = startAt + buffer.duration;
    if (this.context?.state !== 'running') {
      this.updateState('suspended');
    }
  }

  private handleOpus(frame: AudioMediaFrame): void {
    if (this.opusStatus === 'unavailable') {
      return;
    }
    const key = `${frame.sampleRate}:${frame.channels}`;
    if (this.opusStatus === 'ready' && this.opusKey === key && this.opusDecoder) {
      this.decodeOpus(frame);
      return;
    }
    if (this.opusStatus === 'preparing' && this.opusKey === key) {
      if (this.opusQueue.length < 4) {
        this.opusQueue.push(frame);
      }
      return;
    }
    if (this.opusStatus === 'preparing') {
      return;
    }
    void this.prepareOpus(frame);
  }

  private async prepareOpus(frame: AudioMediaFrame): Promise<void> {
    this.opusStatus = 'preparing';
    this.opusKey = `${frame.sampleRate}:${frame.channels}`;
    this.opusQueue = [];
    const channels = Math.min(2, Math.max(1, frame.channels));
    const sampleRate = Math.max(8000, Math.min(48000 * 2, frame.sampleRate || 48000));
    if (typeof AudioDecoder === 'undefined') {
      this.markUnavailable('WebCodecs AudioDecoder is unavailable');
      return;
    }
    const config: AudioDecoderConfig = {
      codec: 'opus',
      sampleRate,
      numberOfChannels: channels,
      description: buildOpusHead(channels, sampleRate),
    };
    try {
      const support = await AudioDecoder.isConfigSupported(config);
      if (!support.supported) {
        this.markUnavailable('Opus is not supported by AudioDecoder');
        return;
      }
      if (this.disposed || this.opusKey !== `${frame.sampleRate}:${frame.channels}`) {
        return;
      }
      const decoder = new AudioDecoder({
        output: (data) => {
          this.playAudioData(data);
        },
        error: (error) => {
          this.markUnavailable(
            error instanceof Error ? error.message : 'Opus decoder error',
          );
        },
      });
      decoder.configure(config);
      this.opusDecoder = decoder;
      this.opusStatus = 'ready';
      const queued = this.opusQueue;
      this.opusQueue = [];
      queued.forEach((queuedFrame) => this.decodeOpus(queuedFrame));
      this.decodeOpus(frame);
    } catch (error) {
      this.markUnavailable(error instanceof Error ? error.message : String(error));
    }
  }

  private decodeOpus(frame: AudioMediaFrame): void {
    if (!this.opusDecoder || this.opusStatus !== 'ready' || this.disposed) {
      return;
    }
    if (this.opusDecoder.decodeQueueSize > 48) {
      return;
    }
    try {
      this.opusDecoder.decode(
        new EncodedAudioChunk({
          type: 'key',
          timestamp: frame.timestampUs,
          data: frame.payload,
        }),
      );
    } catch (error) {
      this.markUnavailable(error instanceof Error ? error.message : String(error));
    }
  }

  private playAudioData(data: AudioData): void {
    if (this.disposed) {
      data.close();
      return;
    }
    const context = this.ensureContext();
    if (!context) {
      data.close();
      return;
    }
    const channels = Math.max(1, data.numberOfChannels);
    const frames = data.numberOfFrames;
    if (frames <= 0) {
      data.close();
      return;
    }
    let buffer: AudioBuffer;
    try {
      buffer = context.createBuffer(channels, frames, data.sampleRate);
      for (let channel = 0; channel < channels; channel += 1) {
        const channelData = new Float32Array(frames);
        data.copyTo(channelData, {
          planeIndex: channel,
          frameOffset: 0,
          frameCount: frames,
          format: 'f32-planar',
        });
        buffer.copyToChannel(channelData, channel);
      }
    } catch (error) {
      data.close();
      this.markUnavailable(error instanceof Error ? error.message : String(error));
      return;
    }
    data.close();
    this.playBuffer(buffer, context);
  }

  private markUnavailable(reason: string): void {
    this.opusStatus = 'unavailable';
    this.updateState('unavailable');
    if (!this.unavailableNotified) {
      this.unavailableNotified = true;
      this.callbacks.onUnavailable?.(reason);
    }
  }

  private updateState(state: AudioPlayerState): void {
    if (this.state === state) {
      return;
    }
    this.state = state;
    this.callbacks.onStateChange?.(state);
  }
}
