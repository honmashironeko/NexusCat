import type { TargetFormat } from '../api/types';

export interface ResolutionPreset {
  id: string;
  width: number;
  height: number;
  experimental: boolean;
}

export interface FpsPreset {
  id: string;
  fps: number;
  experimental: boolean;
}

export const RESOLUTION_PRESETS: ResolutionPreset[] = [
  { id: '720p', width: 1280, height: 720, experimental: false },
  { id: '1080p', width: 1920, height: 1080, experimental: false },
  { id: '1440p', width: 2560, height: 1440, experimental: true },
  { id: '2160p', width: 3840, height: 2160, experimental: true },
];

export const FPS_PRESETS: FpsPreset[] = [
  { id: '30fps', fps: 30, experimental: false },
  { id: '60fps', fps: 60, experimental: false },
  { id: '90fps', fps: 90, experimental: true },
  { id: '120fps', fps: 120, experimental: true },
  { id: '144fps', fps: 144, experimental: true },
];

const BITS_PER_PIXEL = 0.2;
const MIN_BITRATE_KBPS = 4_000;
const MAX_BITRATE_KBPS = 100_000;
const BITRATE_STEP_KBPS = 500;

export function findMatchingResolution(
  target: TargetFormat,
): ResolutionPreset | null {
  return (
    RESOLUTION_PRESETS.find(
      (preset) => preset.width === target.width && preset.height === target.height,
    ) ?? null
  );
}

export function findMatchingFps(target: TargetFormat): FpsPreset | null {
  return FPS_PRESETS.find((preset) => preset.fps === target.fps) ?? null;
}

export function recommendedBitrate(
  width: number,
  height: number,
  fps: number,
): number {
  const raw = (width * height * fps * BITS_PER_PIXEL) / 1000;
  const rounded = Math.round(raw / BITRATE_STEP_KBPS) * BITRATE_STEP_KBPS;
  return Math.min(MAX_BITRATE_KBPS, Math.max(MIN_BITRATE_KBPS, rounded));
}
