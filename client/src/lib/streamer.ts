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
  isCodecResolutionAllowed,
} from './guardrails';
import { createSyntheticScreenStream } from './synthetic';

export interface WebTransportDatagramStream {
  writable: WritableStream<Uint8Array>;
}

export interface WebTransportUnidirectionalStream {
  getWriter(): WritableStreamDefaultWriter<Uint8Array>;
}

export interface WebTransportSession {
  ready: Promise<void>;
  datagrams: WebTransportDatagramStream;
  createUnidirectionalStream(): Promise<WebTransportUnidirectionalStream>;
  close(): void;
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
}

declare global {
  interface Window {
    MediaStreamTrackProcessor?: TrackProcessorConstructor;
    WebTransport?: new (url: string, options?: WebTransportOptions) => WebTransportSession;
  }
}

let transport: WebTransportSession | null = null;
let writer: WritableStreamDefaultWriter<Uint8Array> | null = null;
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
let autoCertHash: string | null = null;
let controlWs: WebSocket | null = null;
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
  const deadline = performance.now() + 5000;
  while (!outputGeometry && performance.now() < deadline) {
    await new Promise<void>(resolve => window.setTimeout(resolve, 100));
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
  if (cleanHex.length === 64) {
    const bytes = new Uint8Array(32);
    for (let i = 0; i < 32; i++) {
      bytes[i] = parseInt(cleanHex.substring(i * 2, i * 2 + 2), 16);
    }
    return bytes;
  }
  return null;
}

async function sendAccessUnit(accessUnit: Uint8Array, seq: number, width: number, height: number, codec: string): Promise<void> {
  const tag = codec === 'H265' ? 'H265' : 'H264';

  if (!uniStreamWriter && transport) {
    const uniStream = await transport.createUnidirectionalStream();
    uniStreamWriter = uniStream.getWriter();
  }
  if (!uniStreamWriter) return;

  const packetLen = 16 + accessUnit.length;
  const totalPayloadBytes = 4 + packetLen;
  const combinedBuf = new Uint8Array(totalPayloadBytes);
  const view = new DataView(combinedBuf.buffer);

  view.setUint32(0, packetLen, false);

  for (let i = 0; i < 4; i++) combinedBuf[4 + i] = tag.charCodeAt(i);
  view.setUint32(8, seq, false);
  view.setUint16(12, 0, false);
  view.setUint16(14, 1, false);
  view.setUint16(16, width, false);
  view.setUint16(18, height, false);

  combinedBuf.set(accessUnit, 20);

  await uniStreamWriter.write(combinedBuf);
}

export async function stopStreaming(): Promise<void> {
  if (!isStreaming && !mediaStream && !transport) {
    setSettingsDisabled(false);
    return;
  }
  isStreaming = false;
  seqNum = 0;

  if (controlWs && controlWs.readyState === WebSocket.OPEN) {
    try {
      controlWs.send(JSON.stringify({ type: 'stop' }));
      log('[CONTROL SOCKET] Sent STOP command to device over independent socket.');
    } catch (e) {}
  }

  if (uniStreamWriter) {
    try {
      const stopPacket = new Uint8Array(20);
      const view = new DataView(stopPacket.buffer);
      view.setUint32(0, 16, false);
      stopPacket[4] = 83;
      stopPacket[5] = 84;
      stopPacket[6] = 79;
      stopPacket[7] = 80;
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

  if (writer) {
    try { writer.releaseLock(); } catch (e) {}
    writer = null;
  }

  if (uniStreamWriter) {
    try { uniStreamWriter.releaseLock(); } catch (e) {}
    uniStreamWriter = null;
  }

  if (transport) {
    try { transport.close(); } catch (e) {}
    transport = null;
  }

  if (!isRemoteStreaming) {
    if (controlWs && controlWs.readyState === WebSocket.OPEN) {
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

  const boardIp = window.location.hostname || '192.168.1.72';
  const boardPort = 4433;
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
  const aspectMode = aspectModeSelect?.value === 'stretch' ? 'stretch' : 'preserve';
  const targetFps = parseInt(fpsSelect.value, 10);
  const selectedCodec = codecSelect.value;
  const wireCodec = selectedCodec.startsWith('H264') ? 'H264' : 'H265';
  const isSWRequested = selectedCodec === 'H264_SW';
  const bitrateSetting = bitrateSelect ? bitrateSelect.value : 'auto';
  const latencySetting = latencySelect ? latencySelect.value : 'ULL';
  let displayGeometry: DisplayGeometry;
  try {
    displayGeometry = await waitForOutputGeometry();
  } catch (geometryErr) {
    const errObj = geometryErr as Error;
    log(`[DISPLAY ERROR] ${errObj.message}`, true);
    await stopStreaming();
    return;
  }

  log(`[CONNECTING] WebTransport -> https://${boardIp}:${boardPort}`);
  updateStatus('connecting', 'CONNECTING');

  try {
    const transportOpts: WebTransportOptions = {};
    if (!autoCertHash) {
      try {
        const res = await fetch('/cert_hash');
        if (res.ok) autoCertHash = (await res.text()).trim();
      } catch (e) {}
    }
    const certBytes = parseCertHash(autoCertHash);
    if (certBytes) {
      transportOpts.serverCertificateHashes = [{
        algorithm: 'sha-256',
        value: certBytes.buffer as ArrayBuffer
      }];
      log(`[CERT] Auto-authenticated device TLS SHA-256 fingerprint`);
    }

    if (!window.WebTransport) {
      throw new Error('WebTransport is not supported in this browser environment');
    }

    transport = new window.WebTransport(`https://${boardIp}:${boardPort}`, transportOpts);
    await transport.ready;
    writer = transport.datagrams.writable.getWriter();
    log(`[WEBTRANSPORT CONNECTED] Session established over QUIC/UDP!`);

    const videoSourceSelect = document.getElementById('videoSource') as HTMLSelectElement;
    const videoSource = videoSourceSelect.value;

    if (videoSource === 'synthetic') {
      const syntheticWidth = 1920;
      const syntheticHeight = 1080;
      log(`[SOURCE] Using Bouncing Orb / Test Pattern (${syntheticWidth}x${syntheticHeight} native @ ${targetFps} FPS)`);
      mediaStream = createSyntheticScreenStream(syntheticWidth, syntheticHeight, targetFps, () => isStreaming);
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
    const rawWidth = trackSettings.width;
    const rawHeight = trackSettings.height;
    if (!rawWidth || !rawHeight) {
      throw new Error('Chrome did not report native capture dimensions');
    }
    // RK3399 HEVC surfaces are macroblock-aligned; 1080p must be encoded as
    // 1920x1088 even though the user-facing preset remains 1920x1080.
    const activeWidth = Math.ceil(selectedWidth / 16) * 16;
    const activeHeight = Math.ceil(selectedHeight / 16) * 16;
    const encodedDimensions = { width: activeWidth, height: activeHeight };
    const encodedResolution = `${activeWidth}x${activeHeight}`;
    const targetBitrate = calculateTargetBitrate(bitrateSetting, wireCodec, activeWidth, targetFps);
    const targetMbps = (targetBitrate / 1_000_000).toFixed(1);
    const webcodecsLatencyMode = (latencySetting === 'quality') ? 'quality' : 'realtime';
    const keyframeInterval = (latencySetting === 'quality')
      ? targetFps * 2
      : (latencySetting === 'balanced' ? targetFps : Math.max(5, Math.floor(targetFps / 2)));
    const encoderModeLabel = (latencySetting === 'quality')
      ? 'High Quality (Buffered)'
      : (latencySetting === 'balanced' ? 'Balanced LAN' : 'ULL (Ultra Low Latency)');

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

    const codecString = getCodecString(wireCodec, activeWidth, targetFps);
    const hardwarePref: HardwareAcceleration = isSWRequested ? 'prefer-software' : 'prefer-hardware';

    // Verify browser hardware acceleration capability
    let isHWAccelerated = false;
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
        isHWAccelerated = supportCheck.config?.hardwareAcceleration === 'prefer-hardware' && !isSWRequested;
      } catch (e) {}
    }

    if (!isSupported) {
      log(`[ERROR] Selected codec ${wireCodec} (${codecString}) is not supported for encoding in this browser.`, true);
      await stopStreaming();
      return;
    }

    if (!isCodecResolutionAllowed(wireCodec, encodedDimensions)) {
      log(`[ERROR] ${wireCodec} output ${encodedResolution} exceeds the 1920x1080 decoder limit; choose a smaller encoder resolution.`, true);
      await stopStreaming();
      return;
    }

    if (selectedCodec === 'H265' && !isHWAccelerated) {
      log(`[ERROR] H.265 software encoding is blocked to prevent heavy CPU usage. Please select H.264.`, true);
      await stopStreaming();
      return;
    }

    if (controlWs && controlWs.readyState === WebSocket.OPEN) {
      try {
        controlWs.send(JSON.stringify({
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
         }));
         log(`[CONTROL SOCKET] Geometry: capture ${rawWidth}x${rawHeight}, encoded ${activeWidth}x${activeHeight}, KMS 100% HDMI signal, content ${contentRect}`);
      } catch (e) {}
    }

    const statEncoderHW = document.getElementById('statEncoderHW');
    if (statEncoderHW) {
      if (isSWRequested) {
        statEncoderHW.textContent = 'SW Emulated (CPU)';
        statEncoderHW.style.color = '#f59e0b';
      } else if (isHWAccelerated) {
        statEncoderHW.textContent = 'HW Accelerated (GPU)';
        statEncoderHW.style.color = '#10b981';
      } else {
        statEncoderHW.textContent = 'SW Emulated (CPU)';
        statEncoderHW.style.color = '#f59e0b';
      }
    }

    log(`[WEBCODECS] Initializing ${wireCodec} Encoder (${codecString}) at ${activeWidth}x${activeHeight} [${isSWRequested ? 'SW CPU' : (isHWAccelerated ? 'HW GPU' : 'SW CPU')}]...`);

    let forceNextKeyframe = false;

    videoEncoder = new VideoEncoder({
      output: async (chunk, metadata) => {
        seqNum++;
        const accessUnit = convertToAnnexB(chunk, metadata, wireCodec, nalCache, seqNum, log);
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

    const keepAliveTimer = setInterval(() => {
      if (!isStreaming) {
        clearInterval(keepAliveTimer);
        return;
      }
      if (controlWs && controlWs.readyState === WebSocket.OPEN) {
        try { controlWs.send(JSON.stringify({ type: 'ping' })); } catch(e) {}
      }
      if (transport && transport.datagrams && transport.datagrams.writable) {
        try {
          const pingWriter = transport.datagrams.writable.getWriter();
          pingWriter.write(new Uint8Array([80, 73, 78, 71]));
          pingWriter.releaseLock();
        } catch(e) {}
      }
    }, 1000);

    const TrackProcessorClass = window.MediaStreamTrackProcessor;
    if (!TrackProcessorClass || !activeVideoTrack) {
      throw new Error('MediaStreamTrackProcessor is not supported or active video track is missing');
    }

    trackProcessor = new TrackProcessorClass({ track: activeVideoTrack });
    trackProcessorReader = trackProcessor.readable.getReader();

    let frameCount = 0;
    let lastFrameTime = 0;
    const minFrameIntervalMs = 1000 / targetFps - 2;

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
        const needKeyFrame = (frameCount <= 5 || frameCount % keyframeInterval === 0 || forceNextKeyframe);
        if (forceNextKeyframe) forceNextKeyframe = false;

        if (videoEncoder && videoEncoder.encodeQueueSize > 8) {
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
          composedFrame.close();
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
      statEdidMax.textContent = `${msg.edid_max_res || '1920x1080'}${fpsStr}`;
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
    if (statLayout) statLayout.textContent = `${msg.aspect_mode || 'preserve'}: ${msg.content_rect}`;
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
      updateStatus('active', 'STREAMING (IN USE)');
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
          const fpsVal = msg.fps || 30;
          statRes.textContent = `${msg.resolution} @ ${fpsVal} FPS`;
        }
      }
    }
  } else if (msg.state === 'IDLE') {
    isRemoteStreaming = false;
    if (!isStreaming) {
      if (controlWs && controlWs.readyState === WebSocket.OPEN) {
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
  const wsProtocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const host = window.location.host || '192.168.1.72:8080';
  const wsUrl = `${wsProtocol}//${host}/ws`;

  updateStatus('connecting', 'CONNECTING...');

  try {
    controlWs = new WebSocket(wsUrl);
    controlWs.onopen = () => {
      log(`[CONTROL SOCKET] Connected to independent command & telemetry channel (${wsUrl})`);
      updateStatus('connected', 'CONNECTED');
      try {
        controlWs?.send(JSON.stringify({ type: 'get_status' }));
      } catch (e) {}
    };
    controlWs.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data) as ServerStatusMessage;
        if (msg.type === 'status') {
           log(`[TELEMETRY] Device State: ${msg.state} | Encoded: ${msg.resolution} | Display: ${msg.display_resolution || 'N/A'} @ ${msg.display_fps || 0}FPS | Frames: ${msg.frames_submitted}`);
          handleServerStatusUpdate(msg);
        } else if (msg.type === 'pong') {
          log(`[CONTROL SOCKET] Received pong from device`);
        }
      } catch (e) {}
    };
    controlWs.onclose = () => {
      log(`[CONTROL SOCKET] Disconnected. Reconnecting in 3s...`, true);
      updateStatus('disconnected', 'DISCONNECTED');
      setTimeout(initControlSocket, 3000);
    };
    controlWs.onerror = () => {
      updateStatus('disconnected', 'DISCONNECTED');
    };
  } catch (e) {
    const errObj = e as Error;
    log(`[CONTROL SOCKET ERROR] ${errObj.message}`, true);
    updateStatus('disconnected', 'DISCONNECTED');
    setTimeout(initControlSocket, 3000);
  }
}
