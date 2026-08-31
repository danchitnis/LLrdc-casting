import { createNalCache, convertToAnnexB, type NalCache } from './annexb';
import {
  calculateCompositorLayout,
  formatContentRect,
  formatPanelContentRect,
  formatSignalContentRect,
  VideoFrameCompositor,
  type AspectMode,
  type DisplayGeometry,
} from './compositor';
import {
  calculateTargetBitrate,
  getCodecString,
  alignEncoderDimensions,
  isCodecResolutionAllowed,
  updateDisplayFpsGuardrails,
} from './guardrails';
import { createSyntheticScreenStream } from './synthetic';
import {
  EncoderTimingTracker,
  LatencySampleCoordinator,
  LatencySmoother,
  monotonicEpochMs,
  type EncoderTiming,
} from './latency';
import StreamWorker from './stream-worker.ts?worker&inline';
import { CongestionController } from './congestion';
import type { StreamWorkerOutboundMessage } from './stream-worker-protocol';
import {
  CERTIFICATE_CONFIG,
  CODEC_RESOLUTION_LIMITS,
  DECODER_LIMITS,
  ENCODER_GUARDRAILS,
  PAIRING_CONFIG,
  STREAM_DEFAULTS,
  TRANSPORT_CONFIG,
} from './config';

export interface WebTransportDatagramStream {
  writable: WritableStream<Uint8Array>;
}

export interface WebTransportUnidirectionalStream {
  getWriter(): WritableStreamDefaultWriter<Uint8Array>;
}

export interface WebTransportBidirectionalStream {
  readable: ReadableStream<Uint8Array>;
  writable: WritableStream<Uint8Array>;
}

export interface WebTransportSession {
  ready: Promise<void>;
  datagrams: WebTransportDatagramStream;
  createUnidirectionalStream(): Promise<WebTransportUnidirectionalStream>;
  createBidirectionalStream(): Promise<WebTransportBidirectionalStream>;
  close(): void;
}

export interface BootstrapConnection {
  ip: string;
  port: number;
  certHash: string;
  code?: string;
  token?: string;
}

export interface WebTransportOptions {
  serverCertificateHashes?: Array<{
    algorithm: string;
    value: ArrayBuffer;
  }>;
}

export interface TrackProcessorInstance {
  readable: ReadableStream<VideoFrame>;
}

export interface TrackProcessorConstructor {
  new (init: { track: MediaStreamTrack }): TrackProcessorInstance;
}

export interface ServerStatusMessage {
  type?: string;
  state?: string;
  resolution?: string;
  display_resolution?: string;
  signal_resolution?: string;
  panel_resolution?: string;
  capture_resolution?: string;
  encoded_resolution?: string;
  aspect_mode?: string;
  content_rect?: string;
  signal_content_rect?: string;
  panel_content_rect?: string;
  display_fps?: number;
  fps?: number;
  bitrate_mbps?: number;
  latency_mode?: string;
  edid_name?: string;
  edid_type?: string;
  edid_max_res?: string;
  edid_max_fps?: number;
  display_max_fps?: number;
  id?: number;
  seq?: number;
  capture_time_ms?: number;
  encode_duration_ms?: number;
  send_start_time_ms?: number;
  receiver_complete_time_ms?: number;
  receiver_queue_ms?: number;
  server_receive_ms?: number;
  server_send_ms?: number;
}

declare global {
  interface Window {
    MediaStreamTrackProcessor?: TrackProcessorConstructor;
    WebTransport?: new (url: string, options?: WebTransportOptions) => WebTransportSession;
    __LLRDC_BOOTSTRAP_CONNECTION__?: BootstrapConnection;
    __LLRDC_BOOTSTRAP_TRANSPORT__?: WebTransportSession;
  }
}

let transport: WebTransportSession | null = null;
let uniStreamWriter: WritableStreamDefaultWriter<Uint8Array> | null = null;
let mediaStream: MediaStream | null = null;
let activeVideoTrack: MediaStreamTrack | null = null;
let trackProcessor: TrackProcessorInstance | null = null;
let trackProcessorReader: ReadableStreamDefaultReader<VideoFrame> | null = null;
let videoEncoder: VideoEncoder | null = null;
let frameCompositor: VideoFrameCompositor | null = null;
let streamWorker: Worker | null = null;
let streamWorkerStopPromise: Promise<void> | null = null;
let streamWorkerStopResolve: (() => void) | null = null;
let outputGeometry: DisplayGeometry | null = null;
let isStreaming = false;
let isStarting = false;
let isRemoteStreaming = false;
let stopStreamingPromise: Promise<void> | null = null;
let seqNum = 0;
let controlWriter: WritableStreamDefaultWriter<Uint8Array> | null = null;
let controlReader: ReadableStreamDefaultReader<Uint8Array> | null = null;
let pairedConnection: BootstrapConnection | null = null;
let nalCache: NalCache = createNalCache();
let pingTimer: number | null = null;
let pingSequence = 0;
let pendingPing: { id: number; sentAt: number; clientSendMs: number } | null = null;
let currentDisplayFps: number = STREAM_DEFAULTS.fps;
const latencySmoother = new LatencySmoother();
const latencyCoordinator = new LatencySampleCoordinator();
interface FrameDiagnostic { accessUnitBytes: number; writeBlockedMs: number; droppedInputFrames: number; configuredBitrateMbps: number; adaptiveBitrateMbps: number; effectiveFps: number; }
const frameDiagnostics = new Map<number, FrameDiagnostic>();
let pingVisibilityHandlerInstalled = false;
type DiagnosticLevel = 'info' | 'warn' | 'error';

interface PendingDiagnostic {
  level: DiagnosticLevel;
  message: string;
}

const MAX_PENDING_DIAGNOSTICS = 100;
const MAX_DIAGNOSTIC_MESSAGE_CHARS = 4096;
const pendingDiagnostics: PendingDiagnostic[] = [];
let diagnosticsFlushActive = false;
const pageSessionId = (typeof crypto !== 'undefined' && 'randomUUID' in crypto)
  ? crypto.randomUUID()
  : `${Date.now()}-${Math.random().toString(16).slice(2)}`;

function getDeviceId(): string {
  try {
    const existing = window.localStorage.getItem('llrdc-device-id');
    if (existing) return existing;
    const created = (typeof crypto !== 'undefined' && 'randomUUID' in crypto)
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
    window.localStorage.setItem('llrdc-device-id', created);
    return created;
  } catch {
    return pageSessionId;
  }
}

function updateLatencyDisplay(value: number | null, detail = 'Available while streaming'): void {
  const stat = document.getElementById('statDevicePing');
  const statDetail = document.getElementById('statLatencyDetail');
  if (stat) stat.textContent = value === null ? '--' : `~${Math.round(value)} ms`;
  if (statDetail) statDetail.textContent = detail;
}

function resetLatencyMetric(measuring = false): void {
  latencySmoother.reset();
  latencyCoordinator.reset();
  frameDiagnostics.clear();
  updateLatencyDisplay(null, measuring ? 'Measuring…' : 'Available while streaming');
}

function reportPendingLatencySample(): void {
  if (!isStreaming || !controlIsConnected()) return;
  const prepared = latencyCoordinator.prepare(monotonicEpochMs(), currentDisplayFps);
  if (!prepared) return;
  const smoothed = latencySmoother.update(prepared.components);
  updateLatencyDisplay(smoothed.totalMs, 'Synchronized encoder-input-to-display estimate');
  const diagnostic = prepared.diagnostics;
  void sendControlMessage({
    type: 'latency_report',
    seq: prepared.seq,
    total_ms: smoothed.totalMs,
    encode_ms: smoothed.encodeMs,
    sender_queue_ms: smoothed.senderQueueMs,
    delivery_ms: smoothed.deliveryMs,
    receiver_queue_ms: smoothed.receiverQueueMs,
    transport_queue_ms: smoothed.senderQueueMs + smoothed.deliveryMs + smoothed.receiverQueueMs,
    decode_display_ms: smoothed.decodeDisplayMs,
    access_unit_bytes: diagnostic.accessUnitBytes ?? 0,
    media_write_blocked_ms: diagnostic.writeBlockedMs ?? 0,
    clock_uncertainty_ms: prepared.clockUncertaintyMs,
    clock_sync_age_ms: prepared.clockAgeMs,
    configured_bitrate_mbps: diagnostic.configuredBitrateMbps ?? 0,
    adaptive_bitrate_mbps: diagnostic.adaptiveBitrateMbps ?? 0,
    dropped_input_frames: diagnostic.droppedInputFrames ?? 0,
    effective_fps: diagnostic.effectiveFps ?? 0,
  }).catch(() => {});
}

function handleLatencySample(message: ServerStatusMessage): void {
  if (!isStreaming || !controlIsConnected()) return;
  if (message.seq === undefined || message.capture_time_ms === undefined || message.encode_duration_ms === undefined
    || message.send_start_time_ms === undefined || message.receiver_complete_time_ms === undefined
    || message.receiver_queue_ms === undefined) return;
  const diagnostic = frameDiagnostics.get(message.seq);
  frameDiagnostics.delete(message.seq);
  latencyCoordinator.acknowledge({
    seq: message.seq,
    captureTimeMs: message.capture_time_ms,
    encodeDurationMs: message.encode_duration_ms,
    sendStartTimeMs: message.send_start_time_ms,
    receiverCompleteTimeMs: message.receiver_complete_time_ms,
    receiverQueueMs: message.receiver_queue_ms,
    ...diagnostic,
  });
  reportPendingLatencySample();
}

function friendlyDiagnostic(message: string): string {
  const normalized = message.toLowerCase();
  if (normalized.includes('pairing') || normalized.includes('receiver code')) {
    return 'Pairing failed. Check the receiver code and try again.';
  }
  if (normalized.includes('capture') || normalized.includes('display error') || normalized.includes('screen capture')) {
    return 'Screen sharing is unavailable or was stopped. Choose a screen and try again.';
  }
  if (normalized.includes('unsupported') || normalized.includes('resolution') || normalized.includes('guardrail')) {
    return 'The selected output is not supported. Choose a lower resolution or another codec.';
  }
  if (normalized.includes('connection') || normalized.includes('webtransport') || normalized.includes('control')) {
    return 'Connection to the receiver was lost. Reconnect and try again.';
  }
  if (normalized.includes('encoder') || normalized.includes('worker') || normalized.includes('track processor')) {
    return 'Casting encountered an encoding problem. Try again or choose H.264.';
  }
  return 'Casting needs attention. Check your connection and try again.';
}

function showUserNotice(message: string, isError = false): void {
  const notice = document.getElementById('userNotice');
  if (!notice) return;
  notice.textContent = message;
  notice.className = `user-notice ${isError ? 'error' : 'info'}`;
  notice.removeAttribute('hidden');
}

function clearUserNotice(): void {
  const notice = document.getElementById('userNotice');
  if (!notice) return;
  notice.textContent = '';
  notice.setAttribute('hidden', '');
}

function queueDiagnostic(level: DiagnosticLevel, message: string): void {
  if (pendingDiagnostics.length >= MAX_PENDING_DIAGNOSTICS) pendingDiagnostics.shift();
  pendingDiagnostics.push({ level, message: message.slice(0, MAX_DIAGNOSTIC_MESSAGE_CHARS) });
  if (controlIsConnected()) void flushDiagnostics();
}

async function flushDiagnostics(): Promise<void> {
  if (diagnosticsFlushActive || !controlIsConnected()) return;
  diagnosticsFlushActive = true;
  try {
    while (controlIsConnected() && pendingDiagnostics.length > 0) {
      const diagnostic = pendingDiagnostics.shift();
      if (!diagnostic) break;
      try {
        await sendControlMessage({ type: 'client_diagnostic', ...diagnostic });
      } catch {
        pendingDiagnostics.unshift(diagnostic);
        break;
      }
    }
  } finally {
    diagnosticsFlushActive = false;
  }
}

function stopPingSampling(resetMetric = true): void {
  if (pingTimer !== null) {
    window.clearInterval(pingTimer);
    pingTimer = null;
  }
  pendingPing = null;
  if (resetMetric) latencyCoordinator.reset();
}

async function sendPing(): Promise<void> {
  if (!controlIsConnected()) return;
  const measureRtt = document.visibilityState === 'visible';
  if (!measureRtt) {
    // Background tabs still need an application-level liveness signal, but
    // the hidden-page RTT is not useful to display and may be heavily
    // timer-throttled by the browser.
    try {
      await sendControlMessage({ type: 'ping' });
    } catch {
      markControlDisconnected();
    }
    return;
  }
  if (pendingPing) {
    if (performance.now() - pendingPing.sentAt <= TRANSPORT_CONFIG.PING_RESPONSE_TIMEOUT_MS) return;
    pendingPing = null;
  }
  const id = pingSequence++;
  const clientSendMs = monotonicEpochMs();
  pendingPing = { id, sentAt: performance.now(), clientSendMs };
  try {
    await sendControlMessage({ type: 'ping', id, client_send_ms: clientSendMs });
  } catch {
    if (pendingPing?.id === id) pendingPing = null;
    markControlDisconnected();
  }
  if (!controlIsConnected() && pendingPing?.id === id) pendingPing = null;
}

function startPingSampling(): void {
  stopPingSampling(false);
  if (!controlIsConnected()) return;
  void sendPing();
  pingTimer = window.setInterval(() => { void sendPing(); }, TRANSPORT_CONFIG.KEEPALIVE_INTERVAL_MS);
}

function handlePong(message: ServerStatusMessage): void {
  if (!pendingPing || (message.id !== undefined && message.id !== pendingPing.id)) return;
  const receivedAtMs = monotonicEpochMs();
  const ping = pendingPing;
  pendingPing = null;
  if (message.server_receive_ms !== undefined && message.server_send_ms !== undefined) {
    const estimate = latencyCoordinator.clock.record(ping.clientSendMs, message.server_receive_ms, message.server_send_ms, receivedAtMs);
    if (estimate) {
      void sendControlMessage({ type: 'clock_sync', offset_ms: estimate.offsetMs, uncertainty_ms: estimate.uncertaintyMs }).catch(() => {});
    }
  }
  reportPendingLatencySample();
}

function markControlDisconnected(): void {
  stopPingSampling();
  resetLatencyMetric();
  controlWriter = null;
  updateStatus('disconnected', 'DISCONNECTED');
  showUserNotice('Connection to the receiver was lost. Reconnect and try again.', true);
}

function parseResolution(value: string | undefined): [number, number] | null {
  if (!value) return null;
  const match = /^(\d+)x(\d+)$/.exec(value.trim());
  if (!match) return null;
  const width = Number.parseInt(match[1], 10);
  const height = Number.parseInt(match[2], 10);
  return width > 0 && height > 0 ? [width, height] : null;
}

async function waitForOutputGeometry(): Promise<DisplayGeometry> {
  const deadline = performance.now() + TRANSPORT_CONFIG.GEOMETRY_TIMEOUT_MS;
  while (!outputGeometry && performance.now() < deadline) {
    await new Promise<void>(resolve => window.setTimeout(resolve, TRANSPORT_CONFIG.GEOMETRY_POLL_INTERVAL_MS));
  }
  if (!outputGeometry) {
    throw new Error('Display geometry is unavailable; waiting for HDMI/EDID telemetry timed out');
  }
  return outputGeometry;
}

export function setSettingsDisabled(disabled: boolean): void {
  const fields = ['videoSource', 'resolution', 'aspectMode', 'fps', 'codec', 'bitrate', 'latencyMode'];
  fields.forEach(id => {
    const el = document.getElementById(id) as HTMLSelectElement | null;
    if (el) el.disabled = disabled;
  });
  const lockNotice = document.getElementById('settingsLockNotice');
  if (lockNotice) {
    lockNotice.style.display = disabled ? 'block' : 'none';
  }
}

export function log(msg: string, isError = false): void {
  const consoleMessage = `[LLRDC UI] ${msg}`;
  if (isError) {
    console.error(consoleMessage);
  } else {
    console.info(consoleMessage);
  }
  queueDiagnostic(isError ? 'error' : 'info', msg);
  if (isError) showUserNotice(friendlyDiagnostic(msg), true);
}

export function updateStatus(state: 'connected' | 'connecting' | 'active' | 'disconnected', label: string): void {
  const badge = document.getElementById('statusBadge');
  const dot = document.getElementById('statusDot');
  if (badge) {
    badge.className = `status-badge ${state}`;
    badge.textContent = label;
  }
  if (dot) {
    dot.className = `status-indicator-dot ${state}`;
  }
}

export function parseCertHash(input: string | null): Uint8Array | null {
  if (!input || !input.trim()) return null;
  const str = input.trim();

  if (str.startsWith('[')) {
    try {
      const arr = JSON.parse(str) as number[];
      return new Uint8Array(arr);
    } catch (e) {}
  }

  const cleanHex = str.replace(/[^0-9a-fA-F]/g, '');
  if (cleanHex.length === CERTIFICATE_CONFIG.SHA256_HEX_LENGTH) {
    const bytes = new Uint8Array(CERTIFICATE_CONFIG.SHA256_DIGEST_BYTES);
    for (let i = 0; i < bytes.length; i++) {
      const start = i * CERTIFICATE_CONFIG.HEX_CHARS_PER_BYTE;
      bytes[i] = parseInt(cleanHex.substring(start, start + CERTIFICATE_CONFIG.HEX_CHARS_PER_BYTE), 16);
    }
    return bytes;
  }
  return null;
}

async function sendAccessUnit(accessUnit: Uint8Array, seq: number, width: number, height: number, codec: string, timing: EncoderTiming | null): Promise<{ senderQueueMs: number; writeBlockedMs: number }> {
  const timedTag = codec === 'H265' ? TRANSPORT_CONFIG.CODEC_TAGS.H265 : TRANSPORT_CONFIG.CODEC_TAGS.H264;
  const legacyTag = codec === 'H265' ? TRANSPORT_CONFIG.LEGACY_CODEC_TAGS.H265 : TRANSPORT_CONFIG.LEGACY_CODEC_TAGS.H264;
  const tag = timing ? timedTag : legacyTag;

  if (!uniStreamWriter && transport) {
    const uniStream = await transport.createUnidirectionalStream();
    uniStreamWriter = uniStream.getWriter();
  }
  if (!uniStreamWriter) return { senderQueueMs: 0, writeBlockedMs: 0 };
  const sendStartTimeMs = monotonicEpochMs();
  const senderQueueMs = timing ? Math.max(0, sendStartTimeMs - timing.captureTimeMs - timing.encodeDurationMs) : 0;

  const headerBytes = timing ? TRANSPORT_CONFIG.PACKET_HEADER_BYTES : TRANSPORT_CONFIG.LEGACY_PACKET_HEADER_BYTES;
  const packetLen = headerBytes + accessUnit.length;
  const totalPayloadBytes = TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + packetLen;
  const combinedBuf = new Uint8Array(totalPayloadBytes);
  const view = new DataView(combinedBuf.buffer);

  view.setUint32(0, packetLen, false);

  const packetOffset = TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES;
  for (let i = 0; i < TRANSPORT_CONFIG.PACKET_FIELD_BYTES.TAG; i++) {
    combinedBuf[packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.TAG + i] = tag.charCodeAt(i);
  }
  view.setUint32(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.SEQUENCE, seq, false);
  view.setUint16(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.CHUNK_INDEX, 0, false);
  view.setUint16(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.CHUNK_COUNT, TRANSPORT_CONFIG.SINGLE_PACKET_CHUNK_COUNT, false);
  view.setUint16(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.WIDTH, width, false);
  view.setUint16(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.HEIGHT, height, false);
  if (timing) {
    view.setFloat64(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.CAPTURE_TIME, timing.captureTimeMs, false);
    view.setFloat32(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.ENCODE_DURATION, timing.encodeDurationMs, false);
    view.setFloat64(packetOffset + TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.SEND_START_TIME, sendStartTimeMs, false);
  }

  combinedBuf.set(accessUnit, TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + headerBytes);

  const writeStartedAt = performance.now();
  await uniStreamWriter.write(combinedBuf);
  return { senderQueueMs, writeBlockedMs: performance.now() - writeStartedAt };
}

function controlIsConnected(): boolean {
  return controlWriter !== null;
}

async function sendControlMessage(message: object): Promise<void> {
  if (!controlWriter) return;
  const payload = new TextEncoder().encode(JSON.stringify(message));
  if (payload.length > TRANSPORT_CONFIG.MAX_CONTROL_MESSAGE_BYTES) {
    throw new Error('Control message exceeds the configured maximum');
  }
  const framed = new Uint8Array(TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + payload.length);
  new DataView(framed.buffer).setUint32(0, payload.length, false);
  framed.set(payload, TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES);
  await controlWriter.write(framed);
}

async function readControlMessages(reader: ReadableStreamDefaultReader<Uint8Array>): Promise<void> {
  let pending = new Uint8Array(0);
  try {
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      const merged = new Uint8Array(pending.length + result.value.length);
      merged.set(pending);
      merged.set(result.value, pending.length);
      pending = merged;
      while (pending.length >= TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES) {
        const length = new DataView(pending.buffer, pending.byteOffset, TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES).getUint32(0, false);
        if (length === 0 || length > TRANSPORT_CONFIG.MAX_CONTROL_MESSAGE_BYTES) throw new Error('Invalid control message length');
        if (pending.length < TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + length) break;
        const payload = pending.slice(TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES, TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + length);
        pending = pending.slice(TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + length);
        const message = JSON.parse(new TextDecoder().decode(payload)) as ServerStatusMessage;
        if (message.type === 'status') handleServerStatusUpdate(message);
        if (message.type === 'pong') handlePong(message);
        if (message.type === 'latency_sample') handleLatencySample(message);
      }
    }
  } catch (error) {
    if (transport) {
      log('[CONTROL] Direct LAN control stream disconnected.', true);
      markControlDisconnected();
    }
  }
  if (transport && controlReader === reader) markControlDisconnected();
  controlReader = null;
  controlWriter = null;
}

async function openPairedTransport(): Promise<void> {
  if (!pairedConnection) throw new Error(`Enter the ${PAIRING_CONFIG.CODE_LENGTH}-character receiver code first`);
  if (!window.WebTransport) throw new Error('WebTransport is not supported in this browser');

  transport = window.__LLRDC_BOOTSTRAP_TRANSPORT__ ?? null;
  delete window.__LLRDC_BOOTSTRAP_TRANSPORT__;
  if (!transport) {
    const certBytes = parseCertHash(pairedConnection.certHash);
    if (!certBytes) throw new Error('Receiver certificate fingerprint is invalid');
    const options: WebTransportOptions = {
      serverCertificateHashes: [{ algorithm: 'sha-256', value: certBytes.buffer as ArrayBuffer }],
    };
    const query = new URLSearchParams();
    if (pairedConnection.code) query.set('code', pairedConnection.code);
    if (pairedConnection.token) query.set('token', pairedConnection.token);
    const url = `https://${pairedConnection.ip}:${pairedConnection.port}/?${query.toString()}`;
    transport = new window.WebTransport(url, options);
  }
  await transport.ready;
  const control = await transport.createBidirectionalStream();
  controlWriter = control.writable.getWriter();
  controlReader = control.readable.getReader();
  void readControlMessages(controlReader);
  await sendControlMessage({
    type: 'client_hello',
    device_id: getDeviceId(),
    user_agent: navigator.userAgent,
    platform: navigator.platform,
    language: navigator.language,
    page_session_id: pageSessionId,
  });
  await flushDiagnostics();
  await sendControlMessage({ type: 'get_status' });
  updateStatus('connected', 'CONNECTED');
  clearUserNotice();
  startPingSampling();
  log('[WEBTRANSPORT] Connected directly to receiver over the LAN.');
}

function isDirectIpPage(): boolean {
  const hostname = window.location.hostname;
  const ipv4 = /^(?:\d{1,3}\.){3}\d{1,3}$/.test(hostname);
  const ipv6 = hostname.includes(':');
  return window.location.port === '8080' || ipv4 || ipv6;
}

export async function pairWithCode(rawCode: string): Promise<void> {
  const code = rawCode.trim().toUpperCase();
  if (!PAIRING_CONFIG.CODE_PATTERN.test(code)) throw new Error(`Enter the ${PAIRING_CONFIG.CODE_LENGTH}-character code shown on the receiver.`);
  updateStatus('connecting', 'PAIRING');
  const localMode = isDirectIpPage();
  if (localMode) {
    const pairingResponse = await fetch('/pairing-config', { cache: 'no-store' });
    const pairingSettings = pairingResponse.ok ? await pairingResponse.json() as { webtransport_port?: number } : {};
    const response = await fetch('/cert_hash', { cache: 'no-store' });
    if (!response.ok) throw new Error('Receiver certificate is unavailable.');
    const certHash = (await response.text()).trim();
    pairedConnection = {
      ip: window.location.hostname,
      port: pairingSettings.webtransport_port || PAIRING_CONFIG.DIRECT_WEBTRANSPORT_PORT,
      certHash,
      code,
    };
  } else {
    const bootstrapped = window.__LLRDC_BOOTSTRAP_CONNECTION__;
    if (bootstrapped?.code === code) {
      pairedConnection = bootstrapped;
      delete window.__LLRDC_BOOTSTRAP_CONNECTION__;
    } else {
      const response = await fetch('/api/pair', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        cache: 'no-store',
        body: JSON.stringify({ code }),
      });
      if (!response.ok) throw new Error('Code invalid or receiver unavailable.');
      const result = await response.json() as {
        ip_address?: string;
        webtransport_port?: number;
        cert_hash_hex?: string;
        connection_token?: string;
      };
      if (!result.ip_address || !result.webtransport_port || !result.cert_hash_hex || !result.connection_token) {
        throw new Error('Pairing response was incomplete.');
      }
      pairedConnection = {
        ip: result.ip_address,
        port: result.webtransport_port,
        certHash: result.cert_hash_hex,
        code,
        token: result.connection_token,
      };
    }
  }
  try {
    await openPairedTransport();
  } catch (error) {
    const reason = error instanceof Error && error.message ? ` (${error.message})` : '';
    log(`[PAIRING] WebTransport connection failed${reason}`, true);
    pairedConnection = null;
    transport = null;
    controlWriter = null;
    controlReader = null;
    stopPingSampling();
    throw new Error(`Receiver is not reachable from this LAN${reason}.`);
  }
  setSettingsDisabled(false);
  const button = document.getElementById('toggleBtn') as HTMLButtonElement | null;
  if (button) button.disabled = false;
  const status = document.getElementById('pairStatus');
  if (status) status.textContent = 'PAIRED';
}

export function initPairing(): void {
  if (!pingVisibilityHandlerInstalled) {
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible') {
        startPingSampling();
      } else {
        // Keep the interval alive while hidden so the receiver can distinguish
        // a background-tab media pause from a disconnected sender. Preserve
        // the last foreground RTT so playback acknowledgements can continue
        // producing portal samples while the user views the management tab.
        pendingPing = null;
        if (controlIsConnected() && pingTimer === null) startPingSampling();
      }
    });
    pingVisibilityHandlerInstalled = true;
  }
  stopPingSampling();
  setSettingsDisabled(true);
  const button = document.getElementById('toggleBtn') as HTMLButtonElement | null;
  if (button) button.disabled = true;
  updateStatus('disconnected', 'ENTER CODE');
  if (isDirectIpPage() && !window.__LLRDC_BOOTSTRAP_CONNECTION__) {
    void fetch('/pairing-config', { cache: 'no-store' })
      .then(async (response) => {
        if (!response.ok) throw new Error('Pairing configuration is unavailable.');
        const config = await response.json() as { required?: boolean; webtransport_port?: number };
        if (config.required !== false) return;
        const certResponse = await fetch('/cert_hash', { cache: 'no-store' });
        if (!certResponse.ok) throw new Error('Receiver certificate is unavailable.');
        pairedConnection = { ip: window.location.hostname, port: config.webtransport_port || PAIRING_CONFIG.DIRECT_WEBTRANSPORT_PORT, certHash: (await certResponse.text()).trim() };
        await openPairedTransport();
        setSettingsDisabled(false);
        const button = document.getElementById('toggleBtn') as HTMLButtonElement | null;
        if (button) button.disabled = false;
        const form = document.getElementById('pairForm');
        if (form) form.hidden = true;
        const help = document.getElementById('pairingHelp');
        if (help) help.textContent = 'Security-code pairing is disabled for this receiver. Video stays on the local network.';
        const status = document.getElementById('pairStatus');
        if (status) status.textContent = 'PAIRED (CODE DISABLED)';
        const notice = document.getElementById('userNotice');
        if (notice) { notice.hidden = false; notice.textContent = 'Security-code pairing is disabled for direct LAN clients.'; notice.className = 'user-notice info'; }
      })
      .catch((error: unknown) => {
        const status = document.getElementById('pairStatus');
        if (status) status.textContent = 'PAIR FAILED';
        log(`[PAIRING] Automatic connection failed: ${error instanceof Error ? error.message : 'unknown error'}`, true);
      });
  }
}

async function stopStreamingOnce(): Promise<void> {
  const wasLocalLifecycle = isStarting || isStreaming || mediaStream !== null
    || activeVideoTrack !== null || streamWorker !== null || videoEncoder !== null;
  if (!isStreaming && !mediaStream && !transport) {
    isStarting = false;
    setSettingsDisabled(false);
    return;
  }
  isStarting = false;
  isStreaming = false;
  resetLatencyMetric();
  seqNum = 0;
  const frameStat = document.getElementById('statFrameCount');
  if (frameStat) frameStat.textContent = '0';
  const outputStat = document.getElementById('statEncodedOutput');
  if (outputStat) outputStat.textContent = 'Not streaming';

  if (controlIsConnected()) {
    try { await sendControlMessage({ type: 'stop' }); } catch (e) {}
  }

  if (streamWorker) {
    const worker = streamWorker;
    const stopWaiter = new Promise<void>((resolve) => { streamWorkerStopResolve = resolve; });
    streamWorkerStopPromise = stopWaiter;
    try {
      worker.postMessage({ type: 'stop' });
      await Promise.race([
        stopWaiter,
        new Promise<void>(resolve => window.setTimeout(resolve, 500)),
      ]);
    } catch (e) {
      log(`[WORKER] Stop handshake failed: ${(e as Error).message}`, true);
    }
    worker.terminate();
    if (streamWorker === worker) streamWorker = null;
    streamWorkerStopPromise = null;
    streamWorkerStopResolve = null;
  }

  if (uniStreamWriter) {
    try {
      const stopPacket = new Uint8Array(TRANSPORT_CONFIG.PACKET_FRAME_PREFIX_BYTES);
      const view = new DataView(stopPacket.buffer);
      view.setUint32(0, TRANSPORT_CONFIG.PACKET_HEADER_BYTES, false);
      for (let i = 0; i < TRANSPORT_CONFIG.PACKET_FIELD_BYTES.TAG; i++) {
        stopPacket[TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES + i] = TRANSPORT_CONFIG.STOP_TAG.charCodeAt(i);
      }
      uniStreamWriter.write(stopPacket).catch(() => {});
    } catch (e) {}
  }

  if (videoEncoder) {
    try { videoEncoder.close(); } catch (e) {}
    videoEncoder = null;
  }
  frameCompositor = null;

  if (trackProcessorReader) {
    const r = trackProcessorReader;
    trackProcessorReader = null;
    try { await r.cancel(); } catch (e) {}
  }
  trackProcessor = null;

  if (activeVideoTrack) {
    try {
      activeVideoTrack.onended = null;
      activeVideoTrack.enabled = false;
      activeVideoTrack.stop();
    } catch (e) {}
    activeVideoTrack = null;
  }

  if (mediaStream) {
    try {
      mediaStream.getTracks().forEach(t => {
        t.onended = null;
        t.enabled = false;
        t.stop();
      });
    } catch (e) {}
    mediaStream = null;
  }

  if (uniStreamWriter) {
    try { uniStreamWriter.releaseLock(); } catch (e) {}
    uniStreamWriter = null;
  }

  // A receiver STREAMING message can already be queued while Chrome fires the
  // capture track's ended event. This tab still owns that teardown, so do not
  // let the queued message leave it rendered as an in-use remote client.
  if (wasLocalLifecycle) isRemoteStreaming = false;
  if (!isRemoteStreaming) {
    if (controlIsConnected()) {
      updateStatus('connected', 'CONNECTED');
    } else {
      updateStatus('disconnected', 'DISCONNECTED');
    }

    const toggleBtn = document.getElementById('toggleBtn');
    const toggleText = document.getElementById('toggleText');
    if (toggleBtn && toggleText) {
      toggleText.textContent = 'Start Casting';
      toggleBtn.className = 'btn-primary';
      (toggleBtn as HTMLButtonElement).disabled = false;
    }

    setSettingsDisabled(false);
  }

  log('[STOPPED] Casting session closed.');
}

export async function stopStreaming(): Promise<void> {
  if (stopStreamingPromise) return stopStreamingPromise;
  const operation = stopStreamingOnce();
  stopStreamingPromise = operation;
  try {
    await operation;
  } finally {
    if (stopStreamingPromise === operation) stopStreamingPromise = null;
  }
}

export async function toggleCasting(): Promise<void> {
  if (isRemoteStreaming || isStarting || stopStreamingPromise) return;
  if (isStreaming) {
    await stopStreaming();
    return;
  }

  isStarting = true;
  isStreaming = true;
  seqNum = 0;
  nalCache = createNalCache();
  resetLatencyMetric(true);

  setSettingsDisabled(true);
  const startingButton = document.getElementById('toggleBtn') as HTMLButtonElement | null;
  const startingText = document.getElementById('toggleText');
  if (startingButton && startingText) {
    startingText.textContent = 'Starting…';
    startingButton.className = 'btn-primary';
    startingButton.disabled = true;
  }

  if (!transport || !controlIsConnected()) {
    log('[PAIRING] Enter the receiver code before starting a cast.', true);
    await stopStreaming();
    return;
  }
  const resSelect = document.getElementById('resolution') as HTMLSelectElement;
  const aspectModeSelect = document.getElementById('aspectMode') as HTMLSelectElement | null;
  const fpsSelect = document.getElementById('fps') as HTMLSelectElement;
  const codecSelect = document.getElementById('codec') as HTMLSelectElement;
  const bitrateSelect = document.getElementById('bitrate') as HTMLSelectElement | null;
  const latencySelect = document.getElementById('latencyMode') as HTMLSelectElement | null;

  const resolution = parseResolution(resSelect.value);
  if (!resolution) {
    log(`[ERROR] Invalid output resolution: ${resSelect.value}`, true);
    await stopStreaming();
    return;
  }
  const [selectedWidth, selectedHeight] = resolution;
  const aspectMode = aspectModeSelect?.value === 'stretch' ? 'stretch' : STREAM_DEFAULTS.aspectMode;
  const targetFps = parseInt(fpsSelect.value, 10);
  const selectedCodec = codecSelect.value;
  const wireCodec = selectedCodec.startsWith('H264') ? 'H264' : 'H265';
  const isSWRequested = selectedCodec === 'H264_SW';
  const bitrateSetting = bitrateSelect ? bitrateSelect.value : STREAM_DEFAULTS.bitrate;
  const latencySetting = latencySelect ? latencySelect.value : STREAM_DEFAULTS.latency;
  let displayGeometry: DisplayGeometry;
  try {
    displayGeometry = await waitForOutputGeometry();
  } catch (geometryErr) {
    const errObj = geometryErr as Error;
    log(`[DISPLAY ERROR] ${errObj.message}`, true);
    await stopStreaming();
    return;
  }

  try {
    const videoSourceSelect = document.getElementById('videoSource') as HTMLSelectElement;
    const videoSource = videoSourceSelect.value;

    // Resolve the selected output before creating the synthetic source so the
    // test pattern and the encoded stream use the same configuration.
    const alignedDimensions = alignEncoderDimensions(wireCodec, selectedWidth, selectedHeight);
    const activeWidth = alignedDimensions.width;
    const activeHeight = alignedDimensions.height;
    const encodedDimensions = { width: activeWidth, height: activeHeight };
    const encodedResolution = `${activeWidth}x${activeHeight}`;
    const targetBitrate = calculateTargetBitrate(bitrateSetting, wireCodec, activeWidth, targetFps);
    const targetMbps = (targetBitrate / 1_000_000).toFixed(1);
    const webcodecsLatencyMode = (latencySetting === 'quality') ? 'quality' : 'realtime';
    const keyframeInterval = (latencySetting === 'quality')
      ? targetFps * 2
      : (latencySetting === 'balanced' ? targetFps : Math.max(ENCODER_GUARDRAILS.MIN_PERIODIC_KEYFRAME_INTERVAL, Math.floor(targetFps / 2)));
    const encoderModeLabel = (latencySetting === 'quality')
      ? 'High Quality (Buffered)'
      : (latencySetting === 'balanced' ? 'Balanced LAN' : 'ULL (Ultra Low Latency)');

    if (videoSource === 'synthetic') {
      log(`[SOURCE] Using Bouncing Orb / Test Pattern (${selectedWidth}x${selectedHeight} native, ${wireCodec} ${isSWRequested ? 'SW' : 'HW preferred'}, ${targetMbps} Mbps, ${aspectMode} @ ${targetFps} FPS)`);
      mediaStream = createSyntheticScreenStream({
        width: selectedWidth,
        height: selectedHeight,
        renderWidth: activeWidth,
        renderHeight: activeHeight,
        encodedWidth: activeWidth,
        encodedHeight: activeHeight,
        fps: targetFps,
        codec: wireCodec,
        hardwarePreference: isSWRequested ? ENCODER_GUARDRAILS.SOFTWARE_ACCELERATION : ENCODER_GUARDRAILS.HARDWARE_ACCELERATION,
        bitrate: targetBitrate,
        aspectMode,
        latencyMode: latencySetting as 'ULL' | 'balanced' | 'quality',
      }, () => isStreaming);
    } else {
      log(`[SOURCE] Requesting full native monitor capture (output ${resSelect.value} @ ${targetFps} FPS)...`);
      try {
        const displayMediaOptions = {
          video: {
            displaySurface: 'monitor',
            frameRate: { ideal: targetFps }
          },
          monitorTypeSurfaces: 'include',
          selfBrowserSurface: 'exclude',
          audio: false
        } as DisplayMediaStreamOptions & {
          monitorTypeSurfaces: 'include';
          selfBrowserSurface: 'exclude';
        };
        mediaStream = await navigator.mediaDevices.getDisplayMedia(displayMediaOptions);
        log(`[SOURCE] Native monitor capture granted.`);
      } catch (captureErr) {
        const errObj = captureErr as Error;
        log(`[SOURCE] Screen capture cancelled or failed: ${errObj.message}`, true);
        await stopStreaming();
        return;
      }
    }

    if (!mediaStream) return;

    activeVideoTrack = mediaStream.getVideoTracks()[0] || null;
    if (activeVideoTrack) {
      activeVideoTrack.onended = () => {
        log('[SCREEN CAPTURE] User stopped casting.');
        stopStreaming();
      };
      activeVideoTrack.onmute = () => log('[SCREEN CAPTURE] Capture track muted by the browser.', true);
      activeVideoTrack.onunmute = () => log('[SCREEN CAPTURE] Capture track resumed.');
    }

    const trackSettings = activeVideoTrack ? activeVideoTrack.getSettings() : {};
    const displaySurface = trackSettings.displaySurface;
    if (videoSource !== 'synthetic' && displaySurface && displaySurface !== 'monitor') {
      throw new Error(`A full monitor is required; browser returned ${displaySurface} capture`);
    }
    const rawWidth = trackSettings.width || activeWidth;
    const rawHeight = trackSettings.height || activeHeight;
    const statRes = document.getElementById('statEncodedOutput');
    const statScale = document.getElementById('statScale');
    const statCodec = document.getElementById('statCodec');
    const statBitrate = document.getElementById('statBitrate');
    const statEncoderMode = document.getElementById('statEncoderMode');
    if (statRes) statRes.textContent = `${encodedResolution} @ ${targetFps} FPS`;
    if (statScale) statScale.textContent = resSelect.value;
    if (statCodec) statCodec.textContent = wireCodec === 'H265' ? 'HEVC / H.265' : (isSWRequested ? 'H.264 (Software Preferred)' : 'H.264');
    if (statBitrate) statBitrate.textContent = `${targetMbps} Mbps (${bitrateSetting === 'auto' ? 'Auto' : 'Custom'})`;
    if (statEncoderMode) statEncoderMode.textContent = encoderModeLabel;

    log(`[CONFIG] Codec: ${wireCodec} (${isSWRequested ? 'SW' : 'HW'}) | Encoder resolution: ${resSelect.value} | Encoded: ${encodedResolution} @ ${targetFps} FPS | Aspect: ${aspectMode} | KMS: 100% HDMI signal | Bandwidth: ${targetMbps} Mbps | Priority: ${encoderModeLabel}`);
    const compositorAspectMode: AspectMode = aspectMode;
    const initialLayout = calculateCompositorLayout(
      rawWidth,
      rawHeight,
      activeWidth,
      activeHeight,
      compositorAspectMode,
      displayGeometry,
    );
    const contentRect = formatContentRect(initialLayout);
    const signalContentRect = formatSignalContentRect(initialLayout);
    const panelContentRect = formatPanelContentRect(initialLayout);

    log(`[SOURCE] Capture dimensions: ${rawWidth}x${rawHeight}${displaySurface ? ` (${displaySurface})` : ''}`);
    log(`[DISPLAY] HDMI signal=${displayGeometry.signalWidth}x${displayGeometry.signalHeight}, panel=${displayGeometry.panelWidth}x${displayGeometry.panelHeight}`);
    log(`[COMPOSITOR] ${aspectMode}: ${rawWidth}x${rawHeight} -> ${activeWidth}x${activeHeight}, encoded=${contentRect}, signal=${signalContentRect}, panel=${panelContentRect}`);
    const statCapture = document.getElementById('statCapture');
    const statEncoded = document.getElementById('statEncoded');
    const statLayout = document.getElementById('statLayout');
    if (statCapture) statCapture.textContent = `${rawWidth}x${rawHeight}`;
    if (statEncoded) statEncoded.textContent = `${activeWidth}x${activeHeight}`;
    if (statLayout) statLayout.textContent = `${aspectMode}: ${contentRect}`;

    try {
      if (typeof OffscreenCanvas === 'undefined') {
        throw new Error('OffscreenCanvas is not supported in this browser');
      }
      frameCompositor = new VideoFrameCompositor(activeWidth, activeHeight, compositorAspectMode, displayGeometry);
    } catch (compositorErr) {
      const errObj = compositorErr as Error;
      log(`[COMPOSITOR ERROR] ${errObj.message}`, true);
      await stopStreaming();
      return;
    }

    const codecString = getCodecString(wireCodec, targetFps);
    const hardwarePref: HardwareAcceleration = isSWRequested
      ? ENCODER_GUARDRAILS.SOFTWARE_ACCELERATION
      : ENCODER_GUARDRAILS.HARDWARE_ACCELERATION;

    // WebCodecs exposes a hardware preference, not portable proof of the
    // backend actually selected by the browser.
    let hardwarePreferenceSupported = false;
    let isSupported = false;
    if (typeof VideoEncoder !== 'undefined' && typeof VideoEncoder.isConfigSupported === 'function') {
      try {
        const supportCheck = await VideoEncoder.isConfigSupported({
          codec: codecString,
          width: activeWidth,
          height: activeHeight,
          bitrate: targetBitrate,
          framerate: targetFps,
          hardwareAcceleration: hardwarePref
        });
        isSupported = !!supportCheck.supported;
        hardwarePreferenceSupported = !!supportCheck.supported && !isSWRequested;
      } catch (e) {}
    }

    if (!isSupported) {
      log(`[ERROR] Selected codec ${wireCodec} (${codecString}) is not supported for encoding in this browser.`, true);
      await stopStreaming();
      return;
    }

    if (!isCodecResolutionAllowed(wireCodec, encodedDimensions)) {
      const maxWidth = wireCodec === 'H265'
        ? CODEC_RESOLUTION_LIMITS.H265_MAX_WIDTH
        : CODEC_RESOLUTION_LIMITS.H264_MAX_WIDTH;
      const maxHeight = wireCodec === 'H265'
        ? CODEC_RESOLUTION_LIMITS.H265_MAX_HEIGHT
        : CODEC_RESOLUTION_LIMITS.H264_MAX_HEIGHT;
      log(`[ERROR] ${wireCodec} output ${encodedResolution} exceeds the ${maxWidth}x${maxHeight} decoder limit; choose a smaller encoder resolution.`, true);
      await stopStreaming();
      return;
    }

    if (controlIsConnected()) {
      try {
        await sendControlMessage({
          type: 'start',
          codec: wireCodec,
          resolution: encodedResolution,
          fps: targetFps,
          bitrate_mbps: parseFloat(targetMbps),
          latency_mode: latencySetting,
          aspect_mode: aspectMode,
          source_width: rawWidth,
          source_height: rawHeight,
          encoded_width: activeWidth,
          encoded_height: activeHeight,
          content_rect: contentRect,
          signal_content_rect: signalContentRect,
          panel_content_rect: panelContentRect,
          signal_width: displayGeometry.signalWidth,
          signal_height: displayGeometry.signalHeight,
          panel_width: displayGeometry.panelWidth,
          panel_height: displayGeometry.panelHeight,
          device_id: getDeviceId(),
        });
          log(`[CONTROL] Geometry: capture ${rawWidth}x${rawHeight}, encoded ${activeWidth}x${activeHeight}, KMS 100% HDMI signal, content ${contentRect}`);
      } catch (e) {}
    }

    const statEncoderHW = document.getElementById('statEncoderHW');
    if (statEncoderHW) {
      if (isSWRequested) {
        statEncoderHW.textContent = 'Software Preferred (Browser API)';
        statEncoderHW.style.color = '#f59e0b';
      } else if (hardwarePreferenceSupported) {
        statEncoderHW.textContent = 'HW Preferred (Browser API)';
        statEncoderHW.style.color = '#10b981';
      } else {
        statEncoderHW.textContent = 'Browser Default (Backend Unknown)';
        statEncoderHW.style.color = '#94a3b8';
      }
    }

    log(`[WEBCODECS] Initializing ${wireCodec} Encoder (${codecString}) at ${activeWidth}x${activeHeight} [${isSWRequested ? 'SW CPU' : (hardwarePreferenceSupported ? 'HW preferred' : 'browser default')}]...`);

    const TrackProcessorClass = window.MediaStreamTrackProcessor;
    let forceNextKeyframe = false;
    const encoderTiming = new EncoderTimingTracker();
    const fallbackCongestion = new CongestionController(latencySetting as 'ULL' | 'balanced' | 'quality', targetBitrate, 1000 / targetFps);
    const fallbackStartedAt = performance.now();
    let fallbackWriteTail: Promise<void> = Promise.resolve();

    // Chromium/Edge can keep the capture processor, encoder, compositor, and
    // WebTransport writer active in a dedicated worker while this page is
    // backgrounded. Safari continues through the main-thread fallback below.
    const chromiumWorkerCandidate = /(?:Chrome|Chromium|Edg\/)/.test(navigator.userAgent);
    if (chromiumWorkerCandidate && TrackProcessorClass && activeVideoTrack && transport) {
      try {
        const processor = new TrackProcessorClass({ track: activeVideoTrack });
        const uniStream = await transport.createUnidirectionalStream();
        const mediaWritable = uniStream as unknown as WritableStream<Uint8Array>;
        const worker = new StreamWorker();
        streamWorker = worker;
        streamWorkerStopPromise = null;
        streamWorkerStopResolve = null;
        worker.onmessage = (event: MessageEvent<StreamWorkerOutboundMessage>) => {
          const message = event.data;
          if (message.type === 'progress' && typeof message.sequence === 'number') {
            seqNum = message.sequence;
            frameDiagnostics.set(message.sequence, {
              accessUnitBytes: message.accessUnitBytes,
              writeBlockedMs: message.writeBlockedMs,
              droppedInputFrames: message.droppedInputFrames,
              configuredBitrateMbps: message.configuredBitrate / 1_000_000,
              adaptiveBitrateMbps: message.adaptiveBitrate / 1_000_000,
              effectiveFps: message.effectiveFps,
            });
            while (frameDiagnostics.size > 64) frameDiagnostics.delete(frameDiagnostics.keys().next().value as number);
            const frameStat = document.getElementById('statFrameCount');
            if (frameStat) frameStat.textContent = message.sequence.toString();
          } else if (message.type === 'log' && message.message) {
            log(message.message, !!message.isError);
          } else if (message.type === 'error' && message.message) {
            log(`[WORKER ERROR] ${message.message}`, true);
            if (streamWorker === worker) {
              worker.terminate();
              streamWorker = null;
              void stopStreaming();
            }
          } else if (message.type === 'stopped') {
            streamWorkerStopResolve?.();
            streamWorkerStopResolve = null;
            if (streamWorkerStopPromise) streamWorkerStopPromise = null;
            if (isStreaming && streamWorker === worker) {
              log('[WORKER] Capture processor ended unexpectedly.', true);
              worker.terminate();
              streamWorker = null;
              void stopStreaming();
            }
          }
        };
        worker.onerror = (event) => {
          log(`[WORKER ERROR] ${event.message || 'Dedicated worker failed'}`, true);
          if (streamWorker === worker) {
            worker.terminate();
            streamWorker = null;
            void stopStreaming();
          }
        };
        worker.postMessage({
          type: 'start',
          readable: processor.readable,
          writable: mediaWritable,
          wireCodec,
          codecString,
          width: activeWidth,
          height: activeHeight,
          bitrate: targetBitrate,
          framerate: targetFps,
          latencyMode: webcodecsLatencyMode as 'quality' | 'realtime',
          congestionMode: latencySetting as 'ULL' | 'balanced' | 'quality',
          hardwareAcceleration: hardwarePref,
          aspectMode: compositorAspectMode,
          displayGeometry,
          keyframeInterval,
        }, [processor.readable as unknown as Transferable, mediaWritable as unknown as Transferable]);
        trackProcessor = processor;
        frameCompositor = null;
        isStarting = false;
        clearUserNotice();
        updateStatus('active', 'STREAMING');
        const toggleBtn = document.getElementById('toggleBtn');
        const toggleText = document.getElementById('toggleText');
        if (toggleBtn && toggleText) {
          toggleText.textContent = 'Stop Casting';
          toggleBtn.className = 'btn-primary stop';
          (toggleBtn as HTMLButtonElement).disabled = false;
        }
        log('[WEBCODECS] Dedicated worker owns capture, encoding, and media transport.');
        return;
      } catch (workerError) {
        streamWorker?.terminate();
        streamWorker = null;
        log(`[WORKER] Dedicated media path unavailable; using main-thread fallback: ${(workerError as Error).message}`, true);
      }
    }

    videoEncoder = new VideoEncoder({
      output: async (chunk, metadata) => {
        seqNum++;
        const outputSeq = seqNum;
        const timing = encoderTiming.resolve(chunk.timestamp);
        const accessUnit = convertToAnnexB(chunk, metadata, wireCodec, nalCache, outputSeq, log);
        if (accessUnit.length > DECODER_LIMITS.MAX_ACCESS_UNIT_BYTES) {
          log(`[GUARDRAIL] Encoded access unit exceeds the ${DECODER_LIMITS.MAX_ACCESS_UNIT_BYTES} byte decoder limit`, true);
          await stopStreaming();
          return;
        }
        fallbackCongestion.writeStarted();
        const writePromise = fallbackWriteTail.then(() => sendAccessUnit(accessUnit, outputSeq, activeWidth, activeHeight, wireCodec, timing));
        fallbackWriteTail = writePromise.then(() => {}, () => {});
        const writeDiagnostic = await writePromise;
        const congestionSnapshot = fallbackCongestion.writeFinished(performance.now(), writeDiagnostic.senderQueueMs, writeDiagnostic.writeBlockedMs);
        if (congestionSnapshot.bitrateChanged && videoEncoder?.state === 'configured') {
          videoEncoder.configure({
            codec: codecString, width: activeWidth, height: activeHeight,
            bitrate: congestionSnapshot.currentBitrate, framerate: targetFps,
            latencyMode: webcodecsLatencyMode as 'quality' | 'realtime', hardwareAcceleration: hardwarePref,
          });
          forceNextKeyframe = true;
        }
        frameDiagnostics.set(outputSeq, {
          accessUnitBytes: accessUnit.length,
          writeBlockedMs: writeDiagnostic.writeBlockedMs,
          droppedInputFrames: congestionSnapshot.droppedInputFrames,
          configuredBitrateMbps: targetBitrate / 1_000_000,
          adaptiveBitrateMbps: congestionSnapshot.currentBitrate / 1_000_000,
          effectiveFps: outputSeq * 1000 / Math.max(1, performance.now() - fallbackStartedAt),
        });
        while (frameDiagnostics.size > 64) frameDiagnostics.delete(frameDiagnostics.keys().next().value as number);

        const frameStat = document.getElementById('statFrameCount');
        if (frameStat) frameStat.textContent = outputSeq.toString();

        if (outputSeq % targetFps === 0) {
          log(`[STREAMING ${wireCodec}] Frame #${outputSeq}: ${activeWidth}x${activeHeight} (${Math.round(accessUnit.length / 1024)} KB) via QUIC stream`);
        }
      },
      error: (e) => {
        log(`[WEBCODECS ERROR] ${e.message}`, true);
        stopStreaming();
      }
    });

    videoEncoder.configure({
      codec: codecString,
      width: activeWidth,
      height: activeHeight,
      bitrate: targetBitrate,
      framerate: targetFps,
      latencyMode: webcodecsLatencyMode as 'quality' | 'realtime',
      hardwareAcceleration: hardwarePref
    });

    clearUserNotice();
    isStarting = false;
    updateStatus('active', 'STREAMING');

    const toggleBtn = document.getElementById('toggleBtn');
    const toggleText = document.getElementById('toggleText');
    if (toggleBtn && toggleText) {
      toggleText.textContent = 'Stop Casting';
      toggleBtn.className = 'btn-primary stop';
      (toggleBtn as HTMLButtonElement).disabled = false;
    }

    const minFrameIntervalMs = 1000 / targetFps - ENCODER_GUARDRAILS.FRAME_TIMING_SLACK_MS;
    if (!TrackProcessorClass || !activeVideoTrack) {
      if (!activeVideoTrack || typeof VideoFrame === 'undefined') {
        throw new Error('Safari fallback requires an active video track and VideoFrame support');
      }

      let fallbackCanvas = document.getElementById('screenCanvas') as HTMLCanvasElement | null;
      if (!fallbackCanvas) {
        fallbackCanvas = document.createElement('canvas');
        fallbackCanvas.id = 'screenCanvas';
        fallbackCanvas.style.display = 'none';
        document.body.appendChild(fallbackCanvas);
      }
      fallbackCanvas.width = rawWidth;
      fallbackCanvas.height = rawHeight;
      const fallbackContext = fallbackCanvas.getContext('2d');
      if (!fallbackContext) throw new Error('Safari fallback could not create a 2D capture canvas');

      let fallbackVideo: HTMLVideoElement | null = null;
      if (videoSource !== 'synthetic') {
        fallbackVideo = document.createElement('video');
        fallbackVideo.muted = true;
        fallbackVideo.autoplay = true;
        fallbackVideo.playsInline = true;
        fallbackVideo.style.display = 'none';
        fallbackVideo.srcObject = mediaStream;
        document.body.appendChild(fallbackVideo);
        try {
          await fallbackVideo.play();
        } catch (playErr) {
          const errObj = playErr as Error;
          throw new Error(`Safari could not play the captured screen: ${errObj.message}`);
        }
        const videoDeadline = performance.now() + 10_000;
        while (fallbackVideo.readyState < HTMLMediaElement.HAVE_CURRENT_DATA && performance.now() < videoDeadline) {
          await new Promise(resolve => setTimeout(resolve, 50));
        }
        if (fallbackVideo.readyState < HTMLMediaElement.HAVE_CURRENT_DATA) {
          throw new Error('Safari did not produce video frames after screen capture permission was granted');
        }
        log('[SOURCE] Safari is rendering the captured monitor through a hidden video canvas.');
      }

      let syntheticFrameCount = 0;
      let lastSyntheticFrameTime = 0;
      try {
        while (isStreaming) {
          await new Promise(resolve => setTimeout(resolve, Math.max(1, minFrameIntervalMs)));
          if (!isStreaming) break;

          const now = performance.now();
          if (lastSyntheticFrameTime > 0 && (now - lastSyntheticFrameTime) < minFrameIntervalMs) continue;
          lastSyntheticFrameTime = now;
          if (fallbackVideo) {
            fallbackContext.drawImage(fallbackVideo, 0, 0, rawWidth, rawHeight);
          }
          const rawFrame = new VideoFrame(fallbackCanvas, { timestamp: Math.round(now * 1000) });
          try {
            if (!videoEncoder || fallbackCongestion.shouldDropInput(videoEncoder.encodeQueueSize, now)) continue;
            syntheticFrameCount++;
            const needKeyFrame = (syntheticFrameCount <= ENCODER_GUARDRAILS.INITIAL_KEYFRAME_COUNT
              || syntheticFrameCount % keyframeInterval === 0 || forceNextKeyframe);
            if (forceNextKeyframe) forceNextKeyframe = false;
            encoderTiming.mark(rawFrame.timestamp);
            const composedFrame = frameCompositor?.compose(rawFrame);
            if (!composedFrame) throw new Error('Video compositor is not initialized');
            videoEncoder.encode(composedFrame, { keyFrame: needKeyFrame });
            if (composedFrame !== rawFrame) composedFrame.close();
          } finally {
            rawFrame.close();
          }
        }
      } finally {
        if (fallbackVideo) {
          fallbackVideo.pause();
          fallbackVideo.srcObject = null;
          fallbackVideo.remove();
        }
      }
      return;
    }

    trackProcessor = new TrackProcessorClass({ track: activeVideoTrack });
    trackProcessorReader = trackProcessor.readable.getReader();

    let frameCount = 0;
    let lastFrameTime = 0;

    while (isStreaming && trackProcessorReader) {
      try {
        const { done, value: rawFrame } = await trackProcessorReader.read();
        if (done || !isStreaming) {
          if (rawFrame) rawFrame.close();
          break;
        }

        const now = performance.now();
        if (lastFrameTime > 0 && (now - lastFrameTime) < minFrameIntervalMs) {
          rawFrame.close();
          continue;
        }
        lastFrameTime = now;

        if (videoEncoder && fallbackCongestion.shouldDropInput(videoEncoder.encodeQueueSize, now)) {
          rawFrame.close();
        } else if (videoEncoder) {
          frameCount++;
          const needKeyFrame = (frameCount <= ENCODER_GUARDRAILS.INITIAL_KEYFRAME_COUNT || frameCount % keyframeInterval === 0 || forceNextKeyframe);
          if (forceNextKeyframe) forceNextKeyframe = false;
          if (frameCount === 1 && frameCompositor) {
            const frameLayout = frameCompositor.layoutFor(rawFrame.displayWidth, rawFrame.displayHeight);
            const visibleRect = rawFrame.visibleRect;
            log(`[FRAME GEOMETRY] VideoFrame coded=${rawFrame.codedWidth}x${rawFrame.codedHeight}, display=${rawFrame.displayWidth}x${rawFrame.displayHeight}, visible=${visibleRect ? `${visibleRect.width}x${visibleRect.height}@${visibleRect.x},${visibleRect.y}` : 'none'}, draw=<${frameLayout.contentX},${frameLayout.contentY},${frameLayout.contentWidth},${frameLayout.contentHeight}>`);
          }
          encoderTiming.mark(rawFrame.timestamp);
          const composedFrame = frameCompositor?.compose(rawFrame);
          if (!composedFrame) {
            rawFrame.close();
            throw new Error('Video compositor is not initialized');
          }
          videoEncoder.encode(composedFrame, { keyFrame: needKeyFrame });
          if (composedFrame !== rawFrame) composedFrame.close();
          rawFrame.close();
        } else {
          rawFrame.close();
        }
      } catch (readErr) {
        if (isStreaming) {
          const errObj = readErr as Error;
          log(`[TRACK PROCESSOR ERROR] ${errObj.message || String(readErr)}`, true);
        }
        break;
      }
    }

    if (isStreaming) {
      log('[TRACK PROCESSOR] Capture readable ended before casting was stopped.', true);
      await stopStreaming();
    }

  } catch (err) {
    const errObj = err as Error;
    log(`[ERROR] ${errObj.message}`, true);
    await stopStreaming();
  }
}

export function handleServerStatusUpdate(msg: ServerStatusMessage): void {
  if (!msg || !msg.state) return;

  // Gate the sender on the active HDMI scanout mode. The EDID/driver maximum
  // is reported separately for diagnostics but does not mean that mode is
  // currently usable by the monitor.
  updateDisplayFpsGuardrails(msg.display_fps ?? msg.display_max_fps ?? msg.edid_max_fps);
  const reportedDisplayFps = msg.display_fps ?? msg.display_max_fps ?? msg.edid_max_fps;
  if (reportedDisplayFps && reportedDisplayFps > 0) currentDisplayFps = reportedDisplayFps;

  const signalResolution = parseResolution(msg.signal_resolution || msg.display_resolution);
  const panelResolution = parseResolution(msg.panel_resolution || msg.edid_max_res);
  if (signalResolution && panelResolution) {
    outputGeometry = {
      signalWidth: signalResolution[0],
      signalHeight: signalResolution[1],
      panelWidth: panelResolution[0],
      panelHeight: panelResolution[1],
    };
    const statSignal = document.getElementById('statSignal');
    const statPanel = document.getElementById('statPanel');
    if (statSignal) statSignal.textContent = `${signalResolution[0]}x${signalResolution[1]}`;
    if (statPanel) statPanel.textContent = `${panelResolution[0]}x${panelResolution[1]}`;
  }

  const toggleBtn = document.getElementById('toggleBtn');
  const toggleText = document.getElementById('toggleText');

  if (msg.edid_name || msg.edid_type) {
    const statEdidName = document.getElementById('statEdidName');
    if (statEdidName) {
      const typeStr = msg.edid_type ? ` (${msg.edid_type})` : '';
      statEdidName.textContent = `${msg.edid_name || 'HDMI Monitor'}${typeStr}`;
    }
  }

  if (msg.edid_max_res || msg.edid_max_fps) {
    const statEdidMax = document.getElementById('statEdidMax');
    if (statEdidMax) {
      const fpsStr = msg.edid_max_fps ? ` @ ${msg.edid_max_fps} FPS` : '';
      statEdidMax.textContent = `${msg.edid_max_res || STREAM_DEFAULTS.resolution}${fpsStr}`;
    }
  }

  if (msg.display_resolution) {
    const statSignal = document.getElementById('statSignal');
    if (statSignal) {
      const fpsStr = msg.display_fps ? ` @ ${msg.display_fps} FPS` : '';
      statSignal.textContent = `${msg.display_resolution}${fpsStr}`;
    }
  }

  if (msg.capture_resolution) {
    const statCapture = document.getElementById('statCapture');
    if (statCapture) statCapture.textContent = msg.capture_resolution;
  }
  if (msg.encoded_resolution) {
    const statEncoded = document.getElementById('statEncoded');
    if (statEncoded) statEncoded.textContent = msg.encoded_resolution;
  }
  if (msg.encoded_resolution) {
    const statScale = document.getElementById('statScale');
    if (statScale) statScale.textContent = msg.encoded_resolution;
  }
  if (msg.content_rect) {
    const statLayout = document.getElementById('statLayout');
    if (statLayout) statLayout.textContent = `${msg.aspect_mode || STREAM_DEFAULTS.aspectMode}: ${msg.content_rect}`;
  }

  if (msg.bitrate_mbps && msg.bitrate_mbps > 0) {
    const statBitrate = document.getElementById('statBitrate');
    if (statBitrate) {
      statBitrate.textContent = `${msg.bitrate_mbps.toFixed(1)} Mbps`;
    }
  }

  if (msg.latency_mode) {
    const statEncoderMode = document.getElementById('statEncoderMode');
    if (statEncoderMode) {
      const modeLabel = (msg.latency_mode === 'quality')
        ? 'High Quality (Buffered)'
        : (msg.latency_mode === 'balanced' ? 'Balanced LAN' : 'ULL (Ultra Low Latency)');
      statEncoderMode.textContent = modeLabel;
    }
  }

  if (msg.state === 'STREAMING') {
    if (isStreaming) {
      isStarting = false;
      clearUserNotice();
      updateStatus('active', 'STREAMING');
      setSettingsDisabled(true);
      if (toggleBtn && toggleText) {
        toggleText.textContent = 'Stop Casting';
        toggleBtn.className = 'btn-primary stop';
        (toggleBtn as HTMLButtonElement).disabled = false;
      }
    } else {
      isRemoteStreaming = true;
      resetLatencyMetric();
      clearUserNotice();
      updateStatus('active', 'IN USE');
      setSettingsDisabled(true);
      if (toggleBtn && toggleText) {
        toggleText.textContent = 'Casting Active (In Use)';
        toggleBtn.className = 'btn-primary stop';
        (toggleBtn as HTMLButtonElement).disabled = true;
      }
    }
    if (msg.resolution && msg.resolution !== '0x0') {
      const statRes = document.getElementById('statEncodedOutput');
      if (statRes) {
        if (msg.resolution.includes('@')) {
          statRes.textContent = msg.resolution;
        } else {
          const fpsVal = msg.fps || STREAM_DEFAULTS.fps;
          statRes.textContent = `${msg.resolution} @ ${fpsVal} FPS`;
        }
      }
    }
  } else if (msg.state === 'IDLE') {
    isRemoteStreaming = false;
    resetLatencyMetric();
    if (isStarting) {
      // The receiver is expected to remain idle while the user is choosing a
      // screen. It becomes active only after capture and encoder setup finish.
      return;
    } else if (!isStreaming) {
      if (controlIsConnected()) {
        updateStatus('connected', 'CONNECTED');
      } else {
        updateStatus('disconnected', 'DISCONNECTED');
      }
      setSettingsDisabled(false);
      if (toggleBtn && toggleText) {
        toggleText.textContent = 'Start Casting';
        toggleBtn.className = 'btn-primary';
        (toggleBtn as HTMLButtonElement).disabled = false;
      }
    } else {
      stopStreaming();
    }
  }
}

export function initControlSocket(): void {
  initPairing();
}
