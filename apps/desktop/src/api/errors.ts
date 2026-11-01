export class ApiError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
  }
}

function readErrorObject(value: object): { code?: unknown; message?: unknown } {
  return value as { code?: unknown; message?: unknown };
}

export function normalizeApiError(value: unknown): ApiError {
  if (value instanceof ApiError) {
    return value;
  }
  if (typeof value === 'string') {
    const trimmed = value.trim();
    if (trimmed.startsWith('{')) {
      try {
        return normalizeApiError(JSON.parse(trimmed) as unknown);
      } catch {
        return new ApiError('tauri_command_failed', trimmed);
      }
    }
    return new ApiError('tauri_command_failed', trimmed || 'Command failed');
  }
  if (typeof value === 'object' && value !== null) {
    const record = readErrorObject(value);
    const code = typeof record.code === 'string' ? record.code : 'tauri_command_failed';
    const message = typeof record.message === 'string' ? record.message : String(value);
    return new ApiError(code, message);
  }
  return new ApiError('tauri_command_failed', String(value));
}

export function errorMessage(value: unknown): string {
  return normalizeApiError(value).message;
}
