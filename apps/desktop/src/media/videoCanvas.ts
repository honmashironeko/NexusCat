export interface CanvasBackingSize {
  width: number;
  height: number;
}

const BACKING_SHRINK_THRESHOLD = 1.2;

export function videoBackingSize(
  displayWidth: number,
  displayHeight: number,
  sourceWidth: number,
  sourceHeight: number,
): CanvasBackingSize {
  const width = Math.max(1, Math.round(displayWidth));
  const height = Math.max(1, Math.round(displayHeight));
  const sourcePixels = Math.max(1, sourceWidth) * Math.max(1, sourceHeight);
  if (width * height <= sourcePixels * BACKING_SHRINK_THRESHOLD) {
    return { width, height };
  }
  const scale = Math.min(
    1,
    Math.max(1, sourceWidth) / width,
    Math.max(1, sourceHeight) / height,
  );
  return {
    width: Math.max(1, Math.round(width * scale)),
    height: Math.max(1, Math.round(height * scale)),
  };
}
