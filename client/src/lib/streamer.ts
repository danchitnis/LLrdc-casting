import { createNalCache, convertToAnnexB, type NalCache } from './annexb';
import { calculateTargetBitrate, getCodecString } from './guardrails';
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
let isStreaming = false;
let isRemoteStreaming = false;
let seqNum = 0;
let autoCertHash: string | null = null;
let controlWs: WebSocket | null = null;
let nalCache: NalCache = createNalCache();

export function setSettingsDisabled(disabled: boolean): void {
  const fields = ['videoSource', 'resolution', 'fps', 'codec', 'bitrate', 'latencyMode'];
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
      toggleText.textContent = 'Start Screen Sharing';
      toggleBtn.className = 'btn-primary';
      (toggleBtn as HTMLButtonElement).disabled = false;
    }

    setSettingsDisabled(false);
  }

  log('[STOPPED] Screen sharing session closed.');
}

export async function toggleScreenShare(): Promise<void> {
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
  const fpsSelect = document.getElementById('fps') as HTMLSelectElement;
  const codecSelect = document.getElementById('codec') as HTMLSelectElement;
  const bitrateSelect = document.getElementById('bitrate') as HTMLSelectElement | null;
  const latencySelect = document.getElementById('latencyMode') as HTMLSelectElement | null;

  const resStr = resSelect.value;
  const targetFps = parseInt(fpsSelect.value, 10);
  const selectedCodec = codecSelect.value;
  const wireCodec = selectedCodec.startsWith('H264') ? 'H264' : 'H265';
  const isSWRequested = selectedCodec === 'H264_SW';
  const bitrateSetting = bitrateSelect ? bitrateSelect.value : 'auto';
  const latencySetting = latencySelect ? latencySelect.value : 'ULL';

  const [width = 1920, height = 1080] = resStr.split('x').map(n => parseInt(n, 10));
  const targetBitrate = calculateTargetBitrate(bitrateSetting, wireCodec, width, targetFps);
  const targetMbps = (targetBitrate / 1_000_000).toFixed(1);

  const webcodecsLatencyMode = (latencySetting === 'quality') ? 'quality' : 'realtime';
  const keyframeInterval = (latencySetting === 'quality')
    ? targetFps * 2
    : (latencySetting === 'balanced' ? targetFps : Math.max(5, Math.floor(targetFps / 2)));

  const encoderModeLabel = (latencySetting === 'quality')
    ? 'High Quality (Buffered)'
    : (latencySetting === 'balanced' ? 'Balanced LAN' : 'ULL (Ultra Low Latency)');

  const statRes = document.getElementById('statResolution');
  const statCodec = document.getElementById('statCodec');
  const statBitrate = document.getElementById('statBitrate');
  const statEncoderMode = document.getElementById('statEncoderMode');
  if (statRes) statRes.textContent = `${resStr} @ ${targetFps} FPS`;
  if (statCodec) statCodec.textContent = wireCodec === 'H265' ? 'HEVC / H.265' : (isSWRequested ? 'H.264 (Software)' : 'H.264');
  if (statBitrate) statBitrate.textContent = `${targetMbps} Mbps (${bitrateSetting === 'auto' ? 'Auto' : 'Custom'})`;
  if (statEncoderMode) statEncoderMode.textContent = encoderModeLabel;

  log(`[CONFIG] Codec: ${wireCodec} (${isSWRequested ? 'SW' : 'HW'}) | Res: ${resStr} @ ${targetFps} FPS | Bandwidth: ${targetMbps} Mbps | Priority: ${encoderModeLabel}`);
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

    if (controlWs && controlWs.readyState === WebSocket.OPEN) {
      try {
        controlWs.send(JSON.stringify({
          type: 'start',
          codec: wireCodec,
          resolution: resStr,
          fps: targetFps,
          bitrate_mbps: parseFloat(targetMbps),
          latency_mode: latencySetting
        }));
      } catch (e) {}
    }

    const videoSourceSelect = document.getElementById('videoSource') as HTMLSelectElement;
    const videoSource = videoSourceSelect.value;

    if (videoSource === 'synthetic') {
      log(`[SOURCE] Using Bouncing Orb / Test Pattern (${width}x${height} @ ${targetFps} FPS)`);
      mediaStream = createSyntheticScreenStream(width, height, targetFps, () => isStreaming);
    } else {
      log(`[SOURCE] Requesting Chrome Screen Capture (${width}x${height} @ ${targetFps} FPS)...`);
      try {
        mediaStream = await navigator.mediaDevices.getDisplayMedia({
          video: {
            width: { ideal: width },
            height: { ideal: height },
            frameRate: { ideal: targetFps }
          },
          audio: false
        });
        log(`[SOURCE] Native Screen Capture granted!`);
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
        log('[SCREEN CAPTURE] User stopped screen sharing.');
        stopStreaming();
      };
    }

    const trackSettings = activeVideoTrack ? activeVideoTrack.getSettings() : {};
    const rawWidth = (videoSource === 'synthetic') ? width : (trackSettings.width || width);
    const rawHeight = (videoSource === 'synthetic') ? height : (trackSettings.height || height);
    const activeWidth = (rawWidth % 16 !== 0) ? Math.ceil(rawWidth / 16) * 16 : rawWidth;
    const activeHeight = (rawHeight % 16 !== 0) ? Math.ceil(rawHeight / 16) * 16 : rawHeight;

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

    if (selectedCodec === 'H265' && !isHWAccelerated) {
      log(`[ERROR] H.265 software encoding is blocked to prevent heavy CPU usage. Please select H.264.`, true);
      await stopStreaming();
      return;
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
      toggleText.textContent = 'Stop Screen Sharing';
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
          videoEncoder.encode(rawFrame, { keyFrame: needKeyFrame });
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
    const statDisplay = document.getElementById('statDisplay');
    if (statDisplay) {
      const fpsStr = msg.display_fps ? ` @ ${msg.display_fps} FPS` : '';
      statDisplay.textContent = `${msg.display_resolution}${fpsStr}`;
    }
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
        toggleText.textContent = 'Stop Screen Sharing';
        toggleBtn.className = 'btn-primary stop';
        (toggleBtn as HTMLButtonElement).disabled = false;
      }
    } else {
      isRemoteStreaming = true;
      updateStatus('active', 'STREAMING (IN USE)');
      setSettingsDisabled(true);
      if (toggleBtn && toggleText) {
        toggleText.textContent = 'Screen Sharing Active (In Use)';
        toggleBtn.className = 'btn-primary stop';
        (toggleBtn as HTMLButtonElement).disabled = true;
      }
    }
    if (msg.resolution && msg.resolution !== '0x0') {
      const statRes = document.getElementById('statResolution');
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
        toggleText.textContent = 'Start Screen Sharing';
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
          log(`[TELEMETRY] Device State: ${msg.state} | Res: ${msg.resolution} | Display: ${msg.display_resolution || 'N/A'} @ ${msg.display_fps || 0}FPS | Frames: ${msg.frames_submitted}`);
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
