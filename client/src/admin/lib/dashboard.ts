export {};

interface ClientMetadata {
  device_id: string;
  user_agent: string;
  platform: string;
  language: string;
  page_session_id: string;
  remote_ip: string;
  connection_id: string;
}

interface ConnectionRecord extends ClientMetadata {
  connected: boolean;
  connected_at_sec: number;
  last_seen_at_sec: number;
  sharing: boolean;
}

interface StreamConfig {
  codec: string;
  resolution: string;
  fps: number;
  bitrate_mbps: number;
  latency_mode: string;
  aspect_mode: string;
  capture_resolution: string;
  encoded_resolution: string;
}

interface MetricSample {
  elapsed_sec: number;
  bitrate_mbps: number;
  fps: number;
}

interface SessionRecord {
  id: number;
  sender: ClientMetadata | null;
  config: StreamConfig;
  duration_sec: number;
  bytes: number;
  end_reason: string | null;
}

interface EventRecord {
  elapsed_sec: number;
  level: string;
  kind: string;
  message: string;
}

interface HealthSnapshot {
  display_resolution: string;
  display_fps: number;
  panel_resolution: string;
  edid_name: string;
  decoder_state: string;
  queue_depth: number;
  dropped_frames: number;
  rejected_frames: number;
  load_average: string;
  memory: string;
  temperature: string;
}

interface ActiveStream {
  id: number;
  sender: ClientMetadata | null;
  config: StreamConfig;
  duration_sec: number;
  frames: number;
  bytes: number;
  measured_bitrate_mbps: number;
  measured_fps: number;
  average_bitrate_mbps: number;
  peak_bitrate_mbps: number;
  sequence_gaps: number;
  server_latency_ms: number;
  samples: MetricSample[];
}

interface ManagementSnapshot {
  server_uptime_sec: number;
  state: string;
  active_stream: ActiveStream | null;
  connections: ConnectionRecord[];
  history: SessionRecord[];
  events: EventRecord[];
  health: HealthSnapshot;
}

interface Snapshot {
  management: ManagementSnapshot;
  pairing: {
    local_status: string;
    cloud_status: string;
  };
  settings: CloudSettingsSnapshot;
}

interface CloudSettingsSnapshot {
  port: number;
  webtransport_port: number;
  http_port: number;
  admin_bind_address: string;
  admin_port: number;
  drm_connector_id: string;
  drm_plane_id: string;
  idle_dashboard: boolean;
  idle_dashboard_mode: string;
  idle_timeout_sec: number;
  sender_liveness_timeout_sec: number;
  udp_buffer_size_mb: number;
  cert_dir: string;
  pairing_worker_url: string;
  receiver_id: string;
  pairing_code_ttl_sec: number;
  local_pairing_code_required: boolean;
  pairing_token_public_key_file: string;
  cloud_discovery_enabled: boolean;
  cloud_configuration_ready: boolean;
  cloud_configuration_missing: string[];
  cloud_state: string;
  pairing_code_source: 'fixed' | 'rotating';
}

const CHART_WINDOW_SEC = 60;
const CHART_HEIGHT = 300;
let points: MetricSample[] = [];
let chartSessionId: number | null = null;
let chartOriginElapsed = 0;
let lastSnapshot: Snapshot | null = null;
let serverUptimeSec = 0;
let pendingRestartValue: boolean | null = null;
let portalDisconnected = false;
let settingsDirty = false;

function element<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`Missing management element #${id}`);
  return node as T;
}

const state = element<HTMLSpanElement>('state');
const stopButton = element<HTMLButtonElement>('stop');
const resetChartButton = element<HTMLButtonElement>('resetChart');
const metrics = element<HTMLDivElement>('metrics');
const sender = element<HTMLParagraphElement>('sender');
const connections = element<HTMLTableSectionElement>('connections');
const historyBody = element<HTMLTableSectionElement>('history');
const health = element<HTMLDivElement>('health');
const events = element<HTMLPreElement>('events');
const chart = element<HTMLCanvasElement>('chart');
const cloudEnabled = element<HTMLInputElement>('cloudEnabled');
const cloudConfig = element<HTMLParagraphElement>('cloudConfig');
const saveSettings = element<HTMLButtonElement>('saveSettings');
const settingsStatus = element<HTMLParagraphElement>('settingsStatus');
const deploymentSettings = element<HTMLDivElement>('deploymentSettings');
const localPairingRequired = element<HTMLInputElement>('localPairingRequired');
const pairingSecuritySource = element<HTMLParagraphElement>('pairingSecuritySource');
const overviewTab = element<HTMLDivElement>('overviewTab');
const settingsTab = element<HTMLDivElement>('settingsTab');
const overviewTabButton = element<HTMLButtonElement>('overviewTabButton');
const settingsTabButton = element<HTMLButtonElement>('settingsTabButton');
const settingInputs = {
  port: element<HTMLInputElement>('settingPort'),
  webtransport_port: element<HTMLInputElement>('settingWebtransportPort'),
  http_port: element<HTMLInputElement>('settingHttpPort'),
  drm_connector_id: element<HTMLInputElement>('settingDrmConnector'),
  drm_plane_id: element<HTMLInputElement>('settingDrmPlane'),
  idle_dashboard: element<HTMLSelectElement>('settingIdleDashboard'),
  idle_dashboard_mode: element<HTMLSelectElement>('settingDashboardMode'),
  idle_timeout_sec: element<HTMLInputElement>('settingIdleTimeout'),
  sender_liveness_timeout_sec: element<HTMLInputElement>('settingLivenessTimeout'),
  udp_buffer_size_mb: element<HTMLInputElement>('settingUdpBuffer'),
  pairing_code_ttl_sec: element<HTMLInputElement>('settingPairingTtl'),
};

function formatBytes(value: number): string {
  if (!value) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  let amount = value;
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024;
    unit += 1;
  }
  return `${amount.toFixed(unit ? 1 : 0)} ${units[unit]}`;
}

function formatTime(elapsedSec: number | null): string {
  if (elapsedSec === null) return '--';
  return new Date(Date.now() - (serverUptimeSec - elapsedSec) * 1000).toLocaleTimeString();
}

function metric(label: string, value: string | number): HTMLDivElement {
  const wrapper = document.createElement('div');
  wrapper.className = 'metric';
  const labelNode = document.createElement('span');
  labelNode.className = 'label';
  labelNode.textContent = label;
  const valueNode = document.createElement('span');
  valueNode.className = 'value';
  valueNode.textContent = String(value);
  wrapper.append(labelNode, valueNode);
  return wrapper;
}

function replaceMetrics(container: HTMLElement, values: ReadonlyArray<readonly [string, string | number]>): void {
  container.replaceChildren(...values.map(([label, value]) => metric(label, value)));
}

function chartScale(value: number): number {
  if (value <= 0) return 1;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  return Math.ceil(value / magnitude) * magnitude;
}

function drawChart(): void {
  const context = chart.getContext('2d');
  if (!context) return;
  const width = chart.clientWidth * 2 || 1000;
  chart.width = width;
  chart.height = CHART_HEIGHT;
  context.clearRect(0, 0, width, CHART_HEIGHT);

  const padding = { left: 66, right: 24, top: 20, bottom: 48 };
  const plotWidth = width - padding.left - padding.right;
  const plotHeight = CHART_HEIGHT - padding.top - padding.bottom;
  const dataMax = Math.max(0, ...points.map((point) => Number.isFinite(point.bitrate_mbps) ? point.bitrate_mbps : 0));
  const max = chartScale(dataMax * 1.1);
  const tickCount = 4;
  const decimals = max < 1 ? 2 : max < 10 ? 1 : 0;

  context.font = '22px system-ui, sans-serif';
  context.lineWidth = 2;
  context.strokeStyle = '#263852';
  context.fillStyle = '#90a4bd';
  context.textAlign = 'right';
  for (let index = 0; index <= tickCount; index += 1) {
    const value = max * index / tickCount;
    const y = padding.top + plotHeight - (value / max) * plotHeight;
    context.beginPath();
    context.moveTo(padding.left, y);
    context.lineTo(width - padding.right, y);
    context.stroke();
    context.fillText(value.toFixed(decimals), padding.left - 10, y + 7);
  }

  context.textAlign = 'center';
  const xTickCount = points.length ? Math.min(3, points.length) : 1;
  for (let index = 0; index < xTickCount; index += 1) {
    const ratio = xTickCount === 1 ? 0 : index / (xTickCount - 1);
    const pointIndex = points.length > 1 ? Math.round(ratio * (points.length - 1)) : 0;
    const x = padding.left + ratio * plotWidth;
    context.fillText(points[pointIndex] ? `${points[pointIndex].elapsed_sec.toFixed(0)}s` : '0s', x, CHART_HEIGHT - 24);
  }
  context.fillStyle = '#b9c8dc';
  context.fillText('Time (rolling 60 seconds)', padding.left + plotWidth / 2, CHART_HEIGHT - 5);
  context.save();
  context.translate(17, padding.top + plotHeight / 2);
  context.rotate(-Math.PI / 2);
  context.fillText('Encoded bitrate (Mbps)', 0, 0);
  context.restore();

  if (points.length < 2) return;
  context.strokeStyle = '#45c2ff';
  context.lineWidth = 4;
  context.beginPath();
  points.forEach((point, index) => {
    const x = padding.left + index * plotWidth / (points.length - 1);
    const y = padding.top + plotHeight - (point.bitrate_mbps / max) * plotHeight;
    if (index) context.lineTo(x, y);
    else context.moveTo(x, y);
  });
  context.stroke();
}

function cell(row: HTMLTableRowElement, primary: string, secondary?: string): void {
  const node = row.insertCell();
  const primaryNode = document.createTextNode(primary);
  node.append(primaryNode);
  if (secondary) {
    node.append(document.createElement('br'));
    const secondaryNode = document.createElement('span');
    secondaryNode.className = 'muted';
    secondaryNode.textContent = secondary;
    node.append(secondaryNode);
  }
}

function emptyRow(body: HTMLTableSectionElement, columns: number, message: string): void {
  const row = body.insertRow();
  const node = row.insertCell();
  node.colSpan = columns;
  node.className = 'muted';
  node.textContent = message;
}

function renderConnections(records: ConnectionRecord[]): void {
  connections.replaceChildren();
  if (!records.length) {
    emptyRow(connections, 5, 'No devices observed');
    return;
  }
  records.forEach((connection) => {
    const row = connections.insertRow();
    cell(row, connection.device_id || 'unknown', connection.page_session_id);
    cell(row, connection.remote_ip || '--');
    cell(row, connection.platform || '--', connection.user_agent);
    cell(row, connection.connected ? (connection.sharing ? 'SHARING' : 'CONNECTED') : 'DISCONNECTED');
    cell(row, formatTime(connection.last_seen_at_sec));
  });
}

function renderHistory(records: SessionRecord[]): void {
  historyBody.replaceChildren();
  if (!records.length) {
    emptyRow(historyBody, 6, 'No completed sessions since startup');
    return;
  }
  records.forEach((record) => {
    const row = historyBody.insertRow();
    cell(row, `#${record.id}`);
    cell(row, record.sender?.device_id || 'unknown', record.sender?.remote_ip);
    cell(row, `${record.config.codec} ${record.config.resolution}`);
    cell(row, `${record.duration_sec.toFixed(1)}s`);
    cell(row, formatBytes(record.bytes));
    cell(row, record.end_reason || '--');
  });
}

function render(snapshot: Snapshot): void {
  lastSnapshot = snapshot;
  const management = snapshot.management;
  const active = management.active_stream;
  serverUptimeSec = management.server_uptime_sec;
  const settings = snapshot.settings;
  if (!settingsDirty && pendingRestartValue === null) cloudEnabled.checked = settings.cloud_discovery_enabled;
  if (!settingsDirty && pendingRestartValue === null) localPairingRequired.checked = settings.local_pairing_code_required;
  if (!settingsDirty && pendingRestartValue === null) populateSettings(settings);
  cloudEnabled.disabled = pendingRestartValue !== null || portalDisconnected;
  localPairingRequired.disabled = pendingRestartValue !== null || portalDisconnected;
  Object.values(settingInputs).forEach((input) => { input.disabled = pendingRestartValue !== null || portalDisconnected; });
  saveSettings.disabled = pendingRestartValue !== null || portalDisconnected || !settingsDirty;
  if (settings.cloud_configuration_ready) {
    cloudConfig.textContent = `Cloudflare provisioning is ready (${settings.cloud_state}). Worker: ${settings.pairing_worker_url}; receiver: ${settings.receiver_id || 'not configured'}.`;
  } else {
    cloudConfig.textContent = `Enablement prerequisites missing: ${settings.cloud_configuration_missing.join(', ')}. Run setup_cloudflare.sh to provision the receiver.`;
  }
  deploymentSettings.replaceChildren(
    metric('Management bind', `${settings.admin_bind_address || '--'}:${settings.admin_port}`),
    metric('Certificate directory', settings.cert_dir),
    metric('Pairing public key', settings.pairing_token_public_key_file),
  );
  pairingSecuritySource.textContent = `Code source: ${settings.pairing_code_source === 'fixed' ? 'fixed deployment code' : 'rotating receiver code'}${settings.local_pairing_code_required ? '.' : '; code enforcement is disabled for direct LAN clients.'}`;
  if (pendingRestartValue !== null && settings.cloud_discovery_enabled === pendingRestartValue && portalDisconnected) {
    pendingRestartValue = null;
    portalDisconnected = false;
    settingsDirty = false;
    cloudEnabled.checked = settings.cloud_discovery_enabled;
    cloudEnabled.disabled = false;
    Object.values(settingInputs).forEach((input) => { input.disabled = false; });
    settingsStatus.textContent = 'Receiver restarted; settings are active.';
  }
  state.textContent = management.state;
  state.style.color = management.state === 'STREAMING' ? '#35d49a' : '#90a4bd';
  stopButton.disabled = !active;

  if (active) {
    replaceMetrics(metrics, [
      ['Codec', active.config.codec],
      ['Resolution', active.config.encoded_resolution || active.config.resolution],
      ['FPS', `${active.measured_fps.toFixed(1)} / ${active.config.fps} target`],
      ['Measured bitrate', `${active.measured_bitrate_mbps.toFixed(2)} Mbps`],
      ['Average bitrate', `${active.average_bitrate_mbps.toFixed(2)} Mbps`],
      ['Peak bitrate', `${active.peak_bitrate_mbps.toFixed(2)} Mbps`],
      ['Frames', active.frames.toLocaleString()],
      ['Bytes', formatBytes(active.bytes)],
      ['Latency', `${active.server_latency_ms.toFixed(1)} ms`],
      ['Sequence gaps', active.sequence_gaps],
    ]);
    const activeSender = active.sender;
    sender.textContent = activeSender
      ? `Active sender: ${activeSender.device_id} · ${activeSender.remote_ip} · ${activeSender.platform} · ${activeSender.user_agent}`
      : 'Active sender metadata pending';
    const samples = active.samples || [];
    const latestElapsed = samples.at(-1)?.elapsed_sec ?? active.duration_sec;
    if (chartSessionId !== active.id || latestElapsed < chartOriginElapsed) {
      chartSessionId = active.id;
      chartOriginElapsed = Math.max(0, latestElapsed - CHART_WINDOW_SEC);
    }
    const windowStart = Math.max(chartOriginElapsed, latestElapsed - CHART_WINDOW_SEC);
    points = samples.filter((sample) => sample.elapsed_sec >= windowStart && sample.elapsed_sec <= latestElapsed).slice(-90);
  } else {
    replaceMetrics(metrics, [['Stream', 'Idle']]);
    sender.textContent = 'No active sender';
    points = [];
    chartSessionId = null;
    chartOriginElapsed = 0;
  }
  drawChart();
  renderConnections(management.connections);
  renderHistory(management.history);

  const receiverHealth = management.health;
  replaceMetrics(health, [
    ['Display', `${receiverHealth.display_resolution} @ ${receiverHealth.display_fps} Hz`],
    ['Panel', receiverHealth.panel_resolution || '--'],
    ['EDID', receiverHealth.edid_name || '--'],
    ['Pairing', snapshot.pairing.local_status],
    ['Cloud', snapshot.pairing.cloud_status],
    ['Decoder', receiverHealth.decoder_state],
    ['Queue', receiverHealth.queue_depth],
    ['Dropped', receiverHealth.dropped_frames],
    ['Rejected', receiverHealth.rejected_frames],
    ['Load', receiverHealth.load_average || '--'],
    ['Memory', receiverHealth.memory || '--'],
    ['Temperature', receiverHealth.temperature || '--'],
  ]);
  events.textContent = management.events.slice(-80)
    .map((event) => `[${event.elapsed_sec.toFixed(1)}s] ${event.level.toUpperCase()} ${event.kind}: ${event.message}`)
    .join('\n') || 'No events';
}

function populateSettings(settings: CloudSettingsSnapshot): void {
  settingInputs.port.value = String(settings.port);
  settingInputs.webtransport_port.value = String(settings.webtransport_port);
  settingInputs.http_port.value = String(settings.http_port);
  settingInputs.drm_connector_id.value = settings.drm_connector_id;
  settingInputs.drm_plane_id.value = settings.drm_plane_id;
  settingInputs.idle_dashboard.value = String(settings.idle_dashboard);
  settingInputs.idle_dashboard_mode.value = settings.idle_dashboard_mode;
  settingInputs.idle_timeout_sec.value = String(settings.idle_timeout_sec);
  settingInputs.sender_liveness_timeout_sec.value = String(settings.sender_liveness_timeout_sec);
  settingInputs.udp_buffer_size_mb.value = String(settings.udp_buffer_size_mb);
  settingInputs.pairing_code_ttl_sec.value = String(settings.pairing_code_ttl_sec);
  localPairingRequired.checked = settings.local_pairing_code_required;
}

function readEditableSettings(): Record<string, unknown> {
  return {
    port: Number(settingInputs.port.value), webtransport_port: Number(settingInputs.webtransport_port.value), http_port: Number(settingInputs.http_port.value),
    drm_connector_id: settingInputs.drm_connector_id.value, drm_plane_id: settingInputs.drm_plane_id.value,
    idle_dashboard: settingInputs.idle_dashboard.value === 'true', idle_dashboard_mode: settingInputs.idle_dashboard_mode.value,
    idle_timeout_sec: Number(settingInputs.idle_timeout_sec.value), sender_liveness_timeout_sec: Number(settingInputs.sender_liveness_timeout_sec.value),
    udp_buffer_size_mb: Number(settingInputs.udp_buffer_size_mb.value), pairing_code_ttl_sec: Number(settingInputs.pairing_code_ttl_sec.value),
    cloud_discovery_enabled: cloudEnabled.checked,
    local_pairing_code_required: localPairingRequired.checked,
  };
}

async function saveAllSettings(): Promise<void> {
  if (!lastSnapshot || !settingsDirty) return;
  if (!confirm(lastSnapshot.management.active_stream ? 'Applying settings restarts the receiver and stops the active share. Continue?' : 'Applying settings restarts the receiver. Continue?')) return;
  saveSettings.disabled = true; cloudEnabled.disabled = true; localPairingRequired.disabled = true;
  settingsStatus.textContent = 'Saving settings; receiver restarting…';
  try {
    const response = await fetch('/api/settings', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ settings: readEditableSettings(), confirm_restart: true }) });
    const payload: unknown = await response.json().catch(() => ({}));
    if (!response.ok) {
      const detail = payload && typeof payload === 'object' && 'missing' in payload && Array.isArray(payload.missing) ? ` Missing: ${(payload.missing as unknown[]).join(', ')}.` : '';
      throw new Error(`Settings were not applied (${response.status}).${detail}`);
    }
    const scheduled = Boolean(payload && typeof payload === 'object' && 'restart_scheduled' in payload && payload.restart_scheduled);
    if (scheduled) {
      pendingRestartValue = Boolean(readEditableSettings().cloud_discovery_enabled);
      settingsStatus.textContent = 'Receiver restarting; waiting for management portal to reconnect…';
    } else {
      settingsDirty = false; settingsStatus.textContent = 'Settings already active.';
    }
  } catch (error) {
    saveSettings.disabled = false; cloudEnabled.disabled = false; localPairingRequired.disabled = false;
    Object.values(settingInputs).forEach((input) => { input.disabled = false; });
    settingsStatus.textContent = error instanceof Error ? error.message : 'Settings update failed.';
  }
}

function selectTab(name: 'overview' | 'settings'): void {
  const settingsSelected = name === 'settings';
  overviewTab.hidden = settingsSelected; settingsTab.hidden = !settingsSelected;
  overviewTabButton.classList.toggle('active', !settingsSelected); settingsTabButton.classList.toggle('active', settingsSelected);
  overviewTabButton.setAttribute('aria-selected', String(!settingsSelected)); settingsTabButton.setAttribute('aria-selected', String(settingsSelected));
}

function isSnapshot(value: unknown): value is Snapshot {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Partial<Snapshot>;
  return Boolean(candidate.management && candidate.pairing && candidate.settings && Array.isArray(candidate.management.connections));
}

function resetChart(): void {
  const active = lastSnapshot?.management.active_stream;
  if (active) {
    const latestElapsed = active.samples.at(-1)?.elapsed_sec ?? active.duration_sec;
    chartSessionId = active.id;
    chartOriginElapsed = latestElapsed;
  } else {
    chartSessionId = null;
    chartOriginElapsed = 0;
  }
  points = [];
  drawChart();
}

async function stopSharing(): Promise<void> {
  if (!confirm('Stop the active share?')) return;
  const response = await fetch('/api/stream/stop', { method: 'POST' });
  if (!response.ok) state.textContent = `STOP FAILED (${response.status})`;
}

function connect(): void {
  const protocol = location.protocol === 'https:' ? 'wss' : 'ws';
  const socket = new WebSocket(`${protocol}://${location.host}/ws`);
  socket.addEventListener('message', (event) => {
    if (typeof event.data !== 'string') return;
    try {
      const parsed: unknown = JSON.parse(event.data);
      if (isSnapshot(parsed)) render(parsed);
    } catch {
      state.textContent = 'INVALID DATA';
    }
  });
  socket.addEventListener('close', () => {
    portalDisconnected = true;
    if (pendingRestartValue !== null) settingsStatus.textContent = 'Receiver restarting; waiting for management portal to reconnect…';
    state.textContent = 'DISCONNECTED';
    window.setTimeout(connect, 1500);
  });
  socket.addEventListener('error', () => socket.close());
}

resetChartButton.addEventListener('click', resetChart);
stopButton.addEventListener('click', () => void stopSharing());
cloudEnabled.addEventListener('change', () => {
  settingsDirty = true;
  saveSettings.disabled = false;
});
localPairingRequired.addEventListener('change', () => {
  settingsDirty = true;
  saveSettings.disabled = false;
});
saveSettings.addEventListener('click', () => void saveAllSettings());
Object.values(settingInputs).forEach((input) => input.addEventListener('input', () => { settingsDirty = true; saveSettings.disabled = false; }));
overviewTabButton.addEventListener('click', () => selectTab('overview'));
settingsTabButton.addEventListener('click', () => selectTab('settings'));
window.addEventListener('resize', drawChart);
connect();
