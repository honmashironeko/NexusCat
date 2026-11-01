export const MEDIA_KIND_VIDEO = 1;
export const MEDIA_KIND_AUDIO = 2;
export const MEDIA_KIND_CONTROL = 3;

const VIDEO_HEADER_BYTES = 36;
const AUDIO_HEADER_BYTES = 20;

export class MediaFrameParseError extends Error {
  readonly reason: string;

  constructor(reason: string) {
    super(reason);
    this.name = 'MediaFrameParseError';
    this.reason = reason;
  }
}

export type AudioCodec = 'opus' | 'pcm-s16le' | 'unknown';

export interface VideoMediaFrame {
  kind: 'video';
  code: typeof MEDIA_KIND_VIDEO;
  streamId: number;
  deviceId: string;
  keyframe: boolean;
  width: number;
  height: number;
  timestampUs: number;
  payload: Uint8Array;
}

export interface AudioMediaFrame {
  kind: 'audio';
  code: typeof MEDIA_KIND_AUDIO;
  codec: AudioCodec;
  codecId: number;
  sampleRate: number;
  channels: number;
  timestampUs: number;
  payload: Uint8Array;
}

export interface ControlMediaFrame {
  kind: 'control';
  code: typeof MEDIA_KIND_CONTROL;
  text: string;
  message: unknown;
}

export interface UnknownMediaFrame {
  kind: 'unknown';
  code: number;
  payload: Uint8Array;
}

export type MediaFrame =
  | VideoMediaFrame
  | AudioMediaFrame
  | ControlMediaFrame
  | UnknownMediaFrame;

function asBytes(data: ArrayBuffer | ArrayBufferView): Uint8Array {
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data);
  }
  return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
}

function readDeviceId(view: DataView, offset: number): string {
  let hex = '';
  for (let index = 0; index < 16; index += 1) {
    hex += view.getUint8(offset + index).toString(16).padStart(2, '0');
  }
  return hex;
}

function decodeCodec(codecId: number): AudioCodec {
  if (codecId === 1) {
    return 'opus';
  }
  if (codecId === 2) {
    return 'pcm-s16le';
  }
  return 'unknown';
}

function parseJson(text: string): unknown {
  if (!text.trim()) {
    return null;
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return null;
  }
}

export function parseMediaFrame(data: ArrayBuffer | ArrayBufferView): MediaFrame {
  const bytes = asBytes(data);
  if (bytes.byteLength === 0) {
    throw new MediaFrameParseError('empty frame');
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const code = bytes[0];

  if (code === MEDIA_KIND_VIDEO) {
    if (bytes.byteLength < VIDEO_HEADER_BYTES) {
      throw new MediaFrameParseError('truncated video header');
    }
    const streamId = view.getUint16(1, true);
    const deviceId = readDeviceId(view, 3);
    const flags = view.getUint8(19);
    const width = view.getUint16(20, true);
    const height = view.getUint16(22, true);
    const timestampUs = Number(view.getBigUint64(24, true));
    const payloadLength = view.getUint32(32, true);
    const payloadStart = VIDEO_HEADER_BYTES;
    if (payloadLength > bytes.byteLength - payloadStart) {
      throw new MediaFrameParseError('truncated video payload');
    }
    return {
      kind: 'video',
      code: MEDIA_KIND_VIDEO,
      streamId,
      deviceId,
      keyframe: (flags & 0x01) !== 0,
      width,
      height,
      timestampUs,
      payload: bytes.slice(payloadStart, payloadStart + payloadLength),
    };
  }

  if (code === MEDIA_KIND_AUDIO) {
    if (bytes.byteLength < AUDIO_HEADER_BYTES) {
      throw new MediaFrameParseError('truncated audio header');
    }
    const codecId = view.getUint8(1);
    const sampleRate = view.getUint32(2, true);
    const channels = view.getUint16(6, true);
    const timestampUs = Number(view.getBigUint64(8, true));
    const payloadLength = view.getUint32(16, true);
    const payloadStart = AUDIO_HEADER_BYTES;
    if (payloadLength > bytes.byteLength - payloadStart) {
      throw new MediaFrameParseError('truncated audio payload');
    }
    return {
      kind: 'audio',
      code: MEDIA_KIND_AUDIO,
      codec: decodeCodec(codecId),
      codecId,
      sampleRate,
      channels,
      timestampUs,
      payload: bytes.slice(payloadStart, payloadStart + payloadLength),
    };
  }

  if (code === MEDIA_KIND_CONTROL) {
    const text = new TextDecoder().decode(bytes.subarray(1));
    return {
      kind: 'control',
      code: MEDIA_KIND_CONTROL,
      text,
      message: parseJson(text),
    };
  }

  return {
    kind: 'unknown',
    code,
    payload: bytes.slice(1),
  };
}

export function isControlMessageType(
  value: unknown,
  type: string,
): value is { type: string; [key: string]: unknown } {
  return (
    typeof value === 'object' &&
    value !== null &&
    (value as { type?: unknown }).type === type
  );
}
