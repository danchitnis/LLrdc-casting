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
  code: string;
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
  frames_submitted?: number;
  edid_name?: string;
  edid_type?: string;
  edid_max_res?: string;
  edid_max_fps?: number;
  display_max_fps?: number;
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
let outputGeometry: DisplayGeometry | null = null;
let isStreaming = false;
let isRemoteStreaming = false;
let seqNum = 0;
let controlWriter: WritableStreamDefaultWriter<Uint8Array> | null = null;
let controlReader: ReadableStreamDefaultReader<Uint8Array> | null = null;
let pairedConnection: BootstrapConnection | null = null;
let nalCache: NalCache = createNalCache();

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

export function clearLogs(): void {
  const logDiv = document.getElementById('log');
  if (logDiv) logDiv.textContent = '';
}

export function log(msg: string, isError = false): void {
  const logDiv = document.getElementById('log');
  if (!logDiv) return;
  const timestamp = new Date().toISOString().split('T')[1].slice(0, 8);
  const line = `[${timestamp}] ${msg}\n`;
  if (isError) {
    const span = document.createElement('span');
    span.style.color = '#f87171';
    span.textContent = line;
    logDiv.appendChild(span);
  } else {
    logDiv.appendChild(document.createTextNode(line));
  }
  logDiv.scrollTop = logDiv.scrollHeight;
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

async function sendAccessUnit(accessUnit: Uint8Array, seq: number, width: number, height: number, codec: string): Promise<void> {
  const tag = codec === 'H265' ? TRANSPORT_CONFIG.CODEC_TAGS.H265 : TRANSPORT_CONFIG.CODEC_TAGS.H264;

  if (!uniStreamWriter && transport) {
    const uniStream = await transport.createUnidirectionalStream();
    uniStreamWriter = uniStream.getWriter();
  }
  if (!uniStreamWriter) return;

  const packetLen = TRANSPORT_CONFIG.PACKET_HEADER_BYTES + accessUnit.length;
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

  combinedBuf.set(accessUnit, TRANSPORT_CONFIG.PACKET_FRAME_PREFIX_BYTES);

  await uniStreamWriter.write(combinedBuf);
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
        if (message.type === 'pong') log('[CONTROL] Received pong from device');
      }
    }
  } catch (error) {
    if (transport) {
      log('[CONTROL] Direct LAN control stream disconnected.', true);
      updateStatus('disconnected', 'DISCONNECTED');
    }
  }
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
    const query = new URLSearchParams({ code: pairedConnection.code });
    if (pairedConnection.token) query.set('token', pairedConnection.token);
    const url = `https://${pairedConnection.ip}:${pairedConnection.port}/?${query.toString()}`;
    transport = new window.WebTransport(url, options);
  }
  await transport.ready;
  const control = await transport.createBidirectionalStream();
  controlWriter = control.writable.getWriter();
  controlReader = control.readable.getReader();
  void readControlMessages(controlReader);
  await sendControlMessage({ type: 'get_status' });
  updateStatus('connected', 'CONNECTED');
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
    const response = await fetch('/cert_hash', { cache: 'no-store' });
    if (!response.ok) throw new Error('Receiver certificate is unavailable.');
    const certHash = (await response.text()).trim();
    pairedConnection = {
      ip: window.location.hostname,
      port: PAIRING_CONFIG.DIRECT_WEBTRANSPORT_PORT,
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
    throw new Error(`Receiver is not reachable from this LAN${reason}.`);
  }
  setSettingsDisabled(false);
  const button = document.getElementById('toggleBtn') as HTMLButtonElement | null;
  if (button) button.disabled = false;
  const status = document.getElementById('pairStatus');
  if (status) status.textContent = 'PAIRED';
}

export function initPairing(): void {
  setSettingsDisabled(true);
  const button = document.getElementById('toggleBtn') as HTMLButtonElement | null;
  if (button) button.disabled = true;
  updateStatus('disconnected', 'ENTER CODE');
}

export async function stopStreaming(): Promise<void> {
  if (!isStreaming && !mediaStream && !transport) {
    setSettingsDisabled(false);
    return;
  }
  isStreaming = false;
  seqNum = 0;

  if (controlIsConnected()) {
    try { await sendControlMessage({ type: 'stop' }); } catch (e) {}
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

export async function toggleCasting(): Promise<void> {
  if (isRemoteStreaming) return;
  if (isStreaming) {
    await stopStreaming();
    return;
  }

  isStreaming = true;
  seqNum = 0;
  nalCache = createNalCache();

  setSettingsDisabled(true);

  if (!transport || !controlIsConnected()) {
    isStreaming = false;
    setSettingsDisabled(false);
    log('[PAIRING] Enter the receiver code before starting a cast.', true);
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
    }

    const trackSettings = activeVideoTrack ? activeVideoTrack.getSettings() : {};
    const displaySurface = trackSettings.displaySurface;
    if (videoSource !== 'synthetic' && displaySurface !== 'monitor') {
      throw new Error(`A full monitor is required; Chrome returned ${displaySurface || 'unknown'} capture`);
    }
    const rawWidth = trackSettings.width || (videoSource === 'synthetic' ? activeWidth : undefined);
    const rawHeight = trackSettings.height || (videoSource === 'synthetic' ? activeHeight : undefined);
    if (!rawWidth || !rawHeight) {
      throw new Error('Chrome did not report native capture dimensions');
    }
    const statRes = document.getElementById('statEncodedOutput');
    const statScale = document.getElementById('statScale');
    const statCodec = document.getElementById('statCodec');
    const statBitrate = document.getElementById('statBitrate');
    const statEncoderMode = document.getElementById('statEncoderMode');
    if (statRes) statRes.textContent = `${encodedResolution} @ ${targetFps} FPS`;
    if (statScale) statScale.textContent = resSelect.value;
    if (statCodec) statCodec.textContent = wireCodec === 'H265' ? 'HEVC / H.265' : (isSWRequested ? 'H.264 (Software)' : 'H.264');
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
         });
          log(`[CONTROL] Geometry: capture ${rawWidth}x${rawHeight}, encoded ${activeWidth}x${activeHeight}, KMS 100% HDMI signal, content ${contentRect}`);
      } catch (e) {}
    }

    const statEncoderHW = document.getElementById('statEncoderHW');
    if (statEncoderHW) {
      if (isSWRequested) {
        statEncoderHW.textContent = 'SW Emulated (CPU)';
        statEncoderHW.style.color = '#f59e0b';
      } else if (hardwarePreferenceSupported) {
        statEncoderHW.textContent = 'HW Preferred (Browser API)';
        statEncoderHW.style.color = '#10b981';
      } else {
        statEncoderHW.textContent = 'SW Emulated (CPU)';
        statEncoderHW.style.color = '#f59e0b';
      }
    }

    log(`[WEBCODECS] Initializing ${wireCodec} Encoder (${codecString}) at ${activeWidth}x${activeHeight} [${isSWRequested ? 'SW CPU' : (hardwarePreferenceSupported ? 'HW preferred' : 'browser default')}]...`);

    let forceNextKeyframe = false;

    videoEncoder = new VideoEncoder({
      output: async (chunk, metadata) => {
        seqNum++;
        const accessUnit = convertToAnnexB(chunk, metadata, wireCodec, nalCache, seqNum, log);
        if (accessUnit.length > DECODER_LIMITS.MAX_ACCESS_UNIT_BYTES) {
          log(`[GUARDRAIL] Encoded access unit exceeds the ${DECODER_LIMITS.MAX_ACCESS_UNIT_BYTES} byte decoder limit`, true);
          await stopStreaming();
          return;
        }
        await sendAccessUnit(accessUnit, seqNum, activeWidth, activeHeight, wireCodec);

        const frameStat = document.getElementById('statFrameCount');
        if (frameStat) frameStat.textContent = seqNum.toString();

        if (seqNum % targetFps === 0) {
          log(`[STREAMING ${wireCodec}] Frame #${seqNum}: ${activeWidth}x${activeHeight} (${Math.round(accessUnit.length / 1024)} KB) via QUIC stream`);
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

    updateStatus('active', 'STREAMING');

    const toggleBtn = document.getElementById('toggleBtn');
    const toggleText = document.getElementById('toggleText');
    if (toggleBtn && toggleText) {
      toggleText.textContent = 'Stop Casting';
      toggleBtn.className = 'btn-primary stop';
    }

    const keepAliveTimer = setInterval(async () => {
      if (!isStreaming) {
        clearInterval(keepAliveTimer);
        return;
      }
      if (controlIsConnected()) {
        try { await sendControlMessage({ type: 'ping' }); } catch(e) {}
      }
      if (transport && transport.datagrams && transport.datagrams.writable) {
        try {
          const pingWriter = transport.datagrams.writable.getWriter();
          pingWriter.write(new Uint8Array([80, 73, 78, 71]));
          pingWriter.releaseLock();
        } catch(e) {}
      }
    }, TRANSPORT_CONFIG.KEEPALIVE_INTERVAL_MS);

    const TrackProcessorClass = window.MediaStreamTrackProcessor;
    if (!TrackProcessorClass || !activeVideoTrack) {
      throw new Error('MediaStreamTrackProcessor is not supported or active video track is missing');
    }

    trackProcessor = new TrackProcessorClass({ track: activeVideoTrack });
    trackProcessorReader = trackProcessor.readable.getReader();

    let frameCount = 0;
    let lastFrameTime = 0;
    const minFrameIntervalMs = 1000 / targetFps - ENCODER_GUARDRAILS.FRAME_TIMING_SLACK_MS;

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

        frameCount++;
        const needKeyFrame = (frameCount <= ENCODER_GUARDRAILS.INITIAL_KEYFRAME_COUNT || frameCount % keyframeInterval === 0 || forceNextKeyframe);
        if (forceNextKeyframe) forceNextKeyframe = false;

        if (videoEncoder && videoEncoder.encodeQueueSize > ENCODER_GUARDRAILS.MAX_ENCODER_QUEUE) {
          rawFrame.close();
        } else if (videoEncoder) {
          if (frameCount === 1 && frameCompositor) {
            const frameLayout = frameCompositor.layoutFor(rawFrame.displayWidth, rawFrame.displayHeight);
            const visibleRect = rawFrame.visibleRect;
            log(`[FRAME GEOMETRY] VideoFrame coded=${rawFrame.codedWidth}x${rawFrame.codedHeight}, display=${rawFrame.displayWidth}x${rawFrame.displayHeight}, visible=${visibleRect ? `${visibleRect.width}x${visibleRect.height}@${visibleRect.x},${visibleRect.y}` : 'none'}, draw=<${frameLayout.contentX},${frameLayout.contentY},${frameLayout.contentWidth},${frameLayout.contentHeight}>`);
          }
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
        break;
      }
    }

    clearInterval(keepAliveTimer);

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
      updateStatus('active', 'STREAMING');
      setSettingsDisabled(true);
      if (toggleBtn && toggleText) {
        toggleText.textContent = 'Stop Casting';
        toggleBtn.className = 'btn-primary stop';
        (toggleBtn as HTMLButtonElement).disabled = false;
      }
    } else {
      isRemoteStreaming = true;
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
    if (!isStreaming) {
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
