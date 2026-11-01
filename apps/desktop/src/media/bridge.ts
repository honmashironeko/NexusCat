import type { MediaBridgeInfo } from '../api/types';
import { parseMediaFrame, type MediaFrame } from './binary';

export type MediaBridgeStatus =
  | 'idle'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'closed'
  | 'error'
  | 'mock';

export interface MediaBridgeHandlers {
  onFrame: (frame: MediaFrame) => void;
  onStatus: (status: MediaBridgeStatus) => void;
  onParseError?: (error: unknown) => void;
}

const MAX_RECONNECT_DELAY_MS = 8000;
const BASE_RECONNECT_DELAY_MS = 450;

export class MediaBridgeClient {
  private readonly handlers: MediaBridgeHandlers;
  private socket: WebSocket | null = null;
  private target: MediaBridgeInfo | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempt = 0;
  private generation = 0;
  private disposed = false;

  constructor(handlers: MediaBridgeHandlers) {
    this.handlers = handlers;
  }

  connect(target: MediaBridgeInfo): void {
    this.target = target;
    this.disposed = false;
    this.reconnectAttempt = 0;
    this.clearReconnectTimer();
    this.openSocket();
  }

  restart(target: MediaBridgeInfo): void {
    this.target = target;
    this.disposed = false;
    this.reconnectAttempt = 0;
    this.clearReconnectTimer();
    this.teardownSocket();
    this.openSocket();
  }

  close(): void {
    this.disposed = true;
    this.generation += 1;
    this.clearReconnectTimer();
    this.teardownSocket();
    this.handlers.onStatus('closed');
  }

  isDisposed(): boolean {
    return this.disposed;
  }

  private openSocket(): void {
    if (this.disposed || !this.target) {
      return;
    }
    if (!this.target.port) {
      this.handlers.onStatus('mock');
      return;
    }
    if (typeof WebSocket === 'undefined') {
      this.handlers.onStatus('error');
      return;
    }
    this.clearReconnectTimer();
    const generation = ++this.generation;
    this.handlers.onStatus(this.reconnectAttempt > 0 ? 'reconnecting' : 'connecting');
    let socket: WebSocket;
    try {
      socket = new WebSocket(
        `ws://127.0.0.1:${this.target.port}/media?token=${encodeURIComponent(this.target.token)}`,
      );
    } catch {
      this.handlers.onStatus('error');
      this.scheduleReconnect();
      return;
    }
    socket.binaryType = 'arraybuffer';
    this.socket = socket;

    socket.onopen = () => {
      if (generation !== this.generation || this.disposed) {
        return;
      }
      this.reconnectAttempt = 0;
      this.handlers.onStatus('connected');
    };

    socket.onmessage = (event: MessageEvent<unknown>) => {
      if (generation !== this.generation || this.disposed) {
        return;
      }
      const data = event.data;
      if (data instanceof ArrayBuffer) {
        this.handleBinary(data);
        return;
      }
      if (typeof Blob !== 'undefined' && data instanceof Blob) {
        void data.arrayBuffer().then((buffer) => {
          if (generation === this.generation && !this.disposed) {
            this.handleBinary(buffer);
          }
        });
      }
    };

    socket.onerror = () => {
      if (generation === this.generation && !this.disposed) {
        this.handlers.onStatus('error');
      }
    };

    socket.onclose = () => {
      if (generation !== this.generation || this.disposed) {
        return;
      }
      this.socket = null;
      this.scheduleReconnect();
    };
  }

  private handleBinary(buffer: ArrayBuffer): void {
    try {
      const frame = parseMediaFrame(buffer);
      this.handlers.onFrame(frame);
    } catch (error) {
      this.handlers.onParseError?.(error);
    }
  }

  private scheduleReconnect(): void {
    if (this.disposed || !this.target || this.reconnectTimer !== null) {
      return;
    }
    this.reconnectAttempt += 1;
    this.handlers.onStatus('reconnecting');
    const exponential = BASE_RECONNECT_DELAY_MS * 2 ** Math.min(this.reconnectAttempt - 1, 5);
    const delay = Math.min(MAX_RECONNECT_DELAY_MS, exponential) + Math.random() * 180;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.openSocket();
    }, delay);
  }

  private clearReconnectTimer(): void {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private teardownSocket(): void {
    if (!this.socket) {
      return;
    }
    this.socket.onopen = null;
    this.socket.onmessage = null;
    this.socket.onerror = null;
    this.socket.onclose = null;
    try {
      this.socket.close();
    } catch {
    }
    this.socket = null;
  }
}
