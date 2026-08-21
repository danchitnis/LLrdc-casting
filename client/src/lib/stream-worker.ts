import { convertToAnnexB, createNalCache } from './annexb';
import { DECODER_LIMITS, ENCODER_GUARDRAILS, TRANSPORT_CONFIG } from './config';
import { VideoFrameCompositor } from './compositor';
import type {
  StreamWorkerMessage,
  StreamWorkerOutboundMessage,
  StreamWorkerStartMessage,
} from './stream-worker-protocol';

interface WorkerScope {
  postMessage(message: StreamWorkerOutboundMessage): void;
  onmessage: ((event: MessageEvent<StreamWorkerMessage>) => void) | null;
}

const workerScope = self as unknown as WorkerScope;

let running = false;
let stopRequested = false;
let activeReader: ReadableStreamDefaultReader<VideoFrame> | null = null;
let activeEncoder: VideoEncoder | null = null;
let activeWriter: WritableStreamDefaultWriter<Uint8Array> | null = null;
let stopPromise: Promise<void> | null = null;
let stopResolve: (() => void) | null = null;

function notifyLog(message: string, isError = false): void {
  workerScope.postMessage({ type: 'log', message, isError });
}

function notifyError(error: unknown): void {
  const message = error instanceof Error ? error.message : String(error);
  workerScope.postMessage({ type: 'error', message });
}

function makePacket(
  accessUnit: Uint8Array,
  sequence: number,
  width: number,
  height: number,
  wireCodec: 'H264' | 'H265',
): Uint8Array {
  const tag = wireCodec === 'H265' ? TRANSPORT_CONFIG.CODEC_TAGS.H265 : TRANSPORT_CONFIG.CODEC_TAGS.H264;
  const packetLen = TRANSPORT_CONFIG.PACKET_HEADER_BYTES + accessUnit.length;
  const totalPayloadBytes = TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + packetLen;
  const packet = new Uint8Array(totalPayloadBytes);
  const view = new DataView(packet.buffer);

  view.setUint32(0, packetLen, false);
  for (let i = 0; i < TRANSPORT_CONFIG.PACKET_FIELD_BYTES.TAG; i++) {
    packet[TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.TAG + i] = tag.charCodeAt(i);
  }
  const packetOffset = TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES;
  view.setUint32(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.SEQUENCE, sequence, false);
  view.setUint16(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.CHUNK_INDEX, 0, false);
  view.setUint16(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.CHUNK_COUNT, TRANSPORT_CONFIG.SINGLE_PACKET_CHUNK_COUNT, false);
  view.setUint16(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.WIDTH, width, false);
  view.setUint16(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.HEIGHT, height, false);
  packet.set(accessUnit, TRANSPORT_CONFIG.PACKET_FRAME_PREFIX_BYTES);
  return packet;
}

function makeStopPacket(): Uint8Array {
  const packet = new Uint8Array(TRANSPORT_CONFIG.PACKET_FRAME_PREFIX_BYTES);
  const view = new DataView(packet.buffer);
  view.setUint32(0, TRANSPORT_CONFIG.PACKET_HEADER_BYTES, false);
  for (let i = 0; i < TRANSPORT_CONFIG.PACKET_FIELD_BYTES.TAG; i++) {
    packet[TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + i] = TRANSPORT_CONFIG.STOP_TAG.charCodeAt(i);
  }
  return packet;
}

async function requestStop(): Promise<void> {
  if (stopPromise) return stopPromise;
  stopRequested = true;
  try { await activeReader?.cancel(); } catch (error) { notifyLog(`[WORKER] Reader cancellation failed: ${String(error)}`, true); }
  stopPromise = new Promise<void>((resolve) => { stopResolve = resolve; });
  return stopPromise;
}

function resolveStopWaiter(): void {
  const resolver = stopResolve as (() => void) | null;
  stopResolve = null;
  if (resolver) resolver();
}

async function startStreaming(message: StreamWorkerStartMessage): Promise<void> {
  if (running) return;
  running = true;
  stopRequested = false;
  stopPromise = null;
  stopResolve = null;

  const nalCache = createNalCache();
  const compositor = new VideoFrameCompositor(message.width, message.height, message.aspectMode, message.displayGeometry);
  const reader = message.readable.getReader();
  const writer = message.writable.getWriter();
  activeReader = reader;
  activeWriter = writer;
  let sequence = 0;
  let frameCount = 0;
  let lastFrameTime = 0;
  const minFrameIntervalMs = 1000 / message.framerate - ENCODER_GUARDRAILS.FRAME_TIMING_SLACK_MS;
  let writeTail = Promise.resolve();

  const writeAccessUnit = (accessUnit: Uint8Array, seq: number): void => {
    writeTail = writeTail.then(async () => {
      if (!activeWriter || stopRequested) return;
      await activeWriter.write(makePacket(accessUnit, seq, message.width, message.height, message.wireCodec));
      workerScope.postMessage({ type: 'progress', sequence: seq, accessUnitBytes: accessUnit.length });
      if (seq % message.framerate === 0) {
        notifyLog(`[STREAMING ${message.wireCodec}] Frame #${seq}: ${message.width}x${message.height} (${Math.round(accessUnit.length / 1024)} KB) via QUIC stream`);
      }
    }).catch((error: unknown) => {
      if (!stopRequested) {
        notifyError(error);
        stopRequested = true;
        void activeReader?.cancel();
      }
    });
  };

  try {
    activeEncoder = new VideoEncoder({
      output: (chunk, metadata) => {
        sequence++;
        const accessUnit = convertToAnnexB(chunk, metadata, message.wireCodec, nalCache, sequence, notifyLog);
        if (accessUnit.length > DECODER_LIMITS.MAX_ACCESS_UNIT_BYTES) {
          notifyError(new Error(`Encoded access unit exceeds the ${DECODER_LIMITS.MAX_ACCESS_UNIT_BYTES} byte decoder limit`));
          stopRequested = true;
          void activeReader?.cancel();
          return;
        }
        writeAccessUnit(accessUnit, sequence);
      },
      error: (error) => notifyError(error),
    });
    activeEncoder.configure({
      codec: message.codecString,
      width: message.width,
      height: message.height,
      bitrate: message.bitrate,
      framerate: message.framerate,
      latencyMode: message.latencyMode,
      hardwareAcceleration: message.hardwareAcceleration,
    });

    while (!stopRequested) {
      const { done, value: rawFrame } = await reader.read();
      if (done || !rawFrame) break;
      try {
        const now = performance.now();
        if (lastFrameTime > 0 && (now - lastFrameTime) < minFrameIntervalMs) continue;
        lastFrameTime = now;
        frameCount++;
        const needKeyFrame = frameCount <= ENCODER_GUARDRAILS.INITIAL_KEYFRAME_COUNT
          || frameCount % message.keyframeInterval === 0;
        if (activeEncoder.encodeQueueSize > ENCODER_GUARDRAILS.MAX_ENCODER_QUEUE) continue;
        const composedFrame = compositor.compose(rawFrame);
        activeEncoder.encode(composedFrame, { keyFrame: needKeyFrame });
        if (composedFrame !== rawFrame) composedFrame.close();
      } finally {
        rawFrame.close();
      }
    }
    if (activeEncoder.state === 'configured') await activeEncoder.flush();
    await writeTail;
  } catch (error) {
    if (!stopRequested) notifyError(error);
  } finally {
    try { activeEncoder?.close(); } catch (error) { notifyLog(`[WORKER] Encoder close failed: ${String(error)}`, true); }
    activeEncoder = null;
    try {
      if (activeWriter) {
        if (stopRequested) await activeWriter.write(makeStopPacket());
        await activeWriter.close();
      }
    } catch (error) {
      if (!stopRequested) notifyError(error);
    }
    activeReader = null;
    activeWriter = null;
    running = false;
    workerScope.postMessage({ type: 'stopped' });
    resolveStopWaiter();
    stopPromise = null;
  }
}

workerScope.onmessage = (event: MessageEvent<StreamWorkerMessage>) => {
  if (event.data.type === 'start') {
    void startStreaming(event.data).catch(notifyError);
  } else if (event.data.type === 'stop') {
    void requestStop();
  }
};
