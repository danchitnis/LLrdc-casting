import type { AspectMode, DisplayGeometry } from './compositor';
import type { CongestionMode } from './congestion';

export interface StreamWorkerStartMessage {
  type: 'start';
  readable: ReadableStream<VideoFrame>;
  writable: WritableStream<Uint8Array>;
  wireCodec: 'H264' | 'H265';
  codecString: string;
  width: number;
  height: number;
  bitrate: number;
  framerate: number;
  latencyMode: 'quality' | 'realtime';
  congestionMode: CongestionMode;
  hardwareAcceleration: HardwareAcceleration;
  aspectMode: AspectMode;
  displayGeometry: DisplayGeometry;
  keyframeInterval: number;
}

export interface StreamWorkerStopMessage {
  type: 'stop';
}

export type StreamWorkerMessage = StreamWorkerStartMessage | StreamWorkerStopMessage;

export interface StreamWorkerProgressMessage {
  type: 'progress';
  sequence: number;
  accessUnitBytes: number;
  senderQueueMs: number;
  writeBlockedMs: number;
  droppedInputFrames: number;
  configuredBitrate: number;
  adaptiveBitrate: number;
  effectiveFps: number;
}

export interface StreamWorkerLogMessage {
  type: 'log';
  message: string;
  isError?: boolean;
}

export interface StreamWorkerErrorMessage {
  type: 'error';
  message: string;
}

export interface StreamWorkerStoppedMessage {
  type: 'stopped';
}

export type StreamWorkerOutboundMessage =
  | StreamWorkerProgressMessage
  | StreamWorkerLogMessage
  | StreamWorkerErrorMessage
  | StreamWorkerStoppedMessage;
