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

interface LatencyMetricSample {
  elapsed_sec: number;
  total_ms: number;
  encode_ms: number;
  sender_queue_ms: number;
  delivery_ms: number;
  receiver_queue_ms: number;
  decode_display_ms: number;
}

type LatencySeriesKey = 'total' | 'encode' | 'sender' | 'delivery' | 'receiver' | 'decode';

interface ChartSeries<T> {
  key: string;
  color: string;
  valueFor: (point: T) => number;
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
  playback_state: string;
  reassembly_in_flight: number;
  dropped_access_units: number;
  ignored_media_packets: number;
  load_1m: number | null;
  load_5m: number | null;
  load_15m: number | null;
  memory_available_mib: number | null;
  memory_total_mib: number | null;
  soc_temperature_c: number | null;
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
  estimated_latency: {
    seq: number;
    total_ms: number;
    encode_ms: number;
    sender_queue_ms: number;
    delivery_ms: number;
    receiver_queue_ms: number;
    transport_queue_ms: number;
    decode_display_ms: number;
    access_unit_bytes: number;
    media_write_blocked_ms: number;
    clock_uncertainty_ms: number;
    clock_sync_age_ms: number;
    configured_bitrate_mbps: number;
    adaptive_bitrate_mbps: number;
    dropped_input_frames: number;
    effective_fps: number;
  } | null;
  estimated_latency_age_ms: number | null;
  samples: MetricSample[];
  latency_samples: LatencyMetricSample[];
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
  watchdog: WatchdogSnapshot;
  build: BuildSnapshot;
  update: UpdateSnapshot;
}

interface BuildSnapshot {
  version: string;
  revision: string;
  built_at: string;
}

interface UpdateSnapshot {
  state: 'idle' | 'checking' | 'current' | 'available' | 'updating' | 'succeeded' | 'failed' | 'rolled_back';
  current_digest: string | null;
  available_digest: string | null;
  current_version: string | null;
  message: string | null;
  updated_at_unix: number | null;
  installed: boolean;
}

interface WatchdogSnapshot {
  manager_uptime_sec: number;
  receiver_state: string;
  receiver_generation: number;
  receiver_pid: number | null;
  receiver_uptime_sec: number | null;
  restart_count: number;
  consecutive_failures: number;
  next_retry_sec: number | null;
  last_failure: string | null;
  logging_healthy: boolean;
  configuration_error: string | null;
}

interface OperationalEvent {
  timestamp_unix_ms: number;
  severity: string;
  category: string;
  message: string;
  receiver_generation: number;
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
  pairing_code_source: 'fixed' | 'rotating' | 'cloud';
}

const CHART_WINDOW_SEC = 60;
const CHART_HEIGHT = 300;
let points: MetricSample[] = [];
let latencyPoints: LatencyMetricSample[] = [];
let chartSessionId: number | null = null;
let chartOriginElapsed = 0;
let lastSnapshot: Snapshot | null = null;
let serverUptimeSec = 0;
let pendingGeneration: number | null = null;
let portalDisconnected = false;
let settingsDirty = false;
let operationalEvents: OperationalEvent[] = [];
const selectedLatencySeries = new Set<LatencySeriesKey>(['total']);

const latencySeries: ReadonlyArray<ChartSeries<LatencyMetricSample> & { key: LatencySeriesKey }> = [
  { key: 'total', color: '#ffc857', valueFor: (point) => point.total_ms },
  { key: 'encode', color: '#45c2ff', valueFor: (point) => point.encode_ms },
  { key: 'sender', color: '#35d49a', valueFor: (point) => point.sender_queue_ms },
  { key: 'delivery', color: '#a78bfa', valueFor: (point) => point.delivery_ms },
  { key: 'receiver', color: '#fb923c', valueFor: (point) => point.receiver_queue_ms },
  { key: 'decode', color: '#ff7eb6', valueFor: (point) => point.decode_display_ms },
];

function element<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`Missing management element #${id}`);
  return node as T;
}

const state = element<HTMLSpanElement>('state');
const stopButton = element<HTMLButtonElement>('stop');
const resetChartButton = element<HTMLButtonElement>('resetChart');
const metrics = element<HTMLDivElement>('metrics');
const latencyMetrics = element<HTMLDivElement>('latencyMetrics');
const congestionMetrics = element<HTMLDivElement>('congestionMetrics');
const measurementMetrics = element<HTMLDivElement>('measurementMetrics');
const sender = element<HTMLParagraphElement>('sender');
const latencyFreshness = element<HTMLParagraphElement>('latencyFreshness');
const connections = element<HTMLTableSectionElement>('connections');
const historyBody = element<HTMLTableSectionElement>('history');
const health = element<HTMLDivElement>('health');
const events = element<HTMLDivElement>('events');
const chart = element<HTMLCanvasElement>('chart');
const latencyChart = element<HTMLCanvasElement>('latencyChart');
const latencySeriesControls = element<HTMLDivElement>('latencySeriesControls');
const watchdog = element<HTMLDivElement>('watchdog');
const watchdogStatus = element<HTMLParagraphElement>('watchdogStatus');
const restartReceiver = element<HTMLButtonElement>('restartReceiver');
const checkUpdate = element<HTMLButtonElement>('checkUpdate');
const applyUpdate = element<HTMLButtonElement>('applyUpdate');
const updateMetrics = element<HTMLDivElement>('updateMetrics');
const updateStatus = element<HTMLParagraphElement>('updateStatus');
const operationalLogs = element<HTMLDivElement>('operationalLogs');
const logSeverity = element<HTMLSelectElement>('logSeverity');
const logCategory = element<HTMLSelectElement>('logCategory');
const logSearch = element<HTMLInputElement>('logSearch');
const logCount = element<HTMLSpanElement>('logCount');
const jumpLatest = element<HTMLButtonElement>('jumpLatest');
const downloadLogs = element<HTMLButtonElement>('downloadLogs');
const cloudEnabled = element<HTMLInputElement>('cloudEnabled');
const cloudConfig = element<HTMLParagraphElement>('cloudConfig');
const saveSettings = element<HTMLButtonElement>('saveSettings');
const settingsStatus = element<HTMLParagraphElement>('settingsStatus');
const deploymentSettings = element<HTMLDivElement>('deploymentSettings');
const localPairingRequired = element<HTMLInputElement>('localPairingRequired');
const pairingSecuritySource = element<HTMLParagraphElement>('pairingSecuritySource');
const overviewTab = element<HTMLDivElement>('overviewTab');
const logsTab = element<HTMLDivElement>('logsTab');
const settingsTab = element<HTMLDivElement>('settingsTab');
const overviewTabButton = element<HTMLButtonElement>('overviewTabButton');
const logsTabButton = element<HTMLButtonElement>('logsTabButton');
const settingsTabButton = element<HTMLButtonElement>('settingsTabButton');
const metricTooltip = element<HTMLDivElement>('metricTooltip');
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

function formatLoad(health: HealthSnapshot): string {
  const values = [health.load_1m, health.load_5m, health.load_15m];
  return values.every((value) => typeof value === 'number' && Number.isFinite(value))
    ? `${values.map((value) => (value as number).toFixed(2)).join(' / ')} (1/5/15m)`
    : '--';
}

function formatMemory(health: HealthSnapshot): string {
  return typeof health.memory_available_mib === 'number' && typeof health.memory_total_mib === 'number'
    ? `${health.memory_available_mib} / ${health.memory_total_mib} MiB`
    : '--';
}

function formatPlaybackState(value: string): string {
  return value ? value.replaceAll('_', ' ').toUpperCase() : '--';
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
  valueNode.dataset.fullValue = String(value);
  wrapper.append(labelNode, valueNode);
  return wrapper;
}

function replaceMetrics(container: HTMLElement, values: ReadonlyArray<readonly [string, string | number]>): void {
  const existing = [...container.querySelectorAll<HTMLElement>(':scope > .metric')];
  const canUpdate = existing.length === values.length && existing.every((item, index) =>
    item.querySelector('.label')?.textContent === values[index][0]
  );
  if (!canUpdate) {
    if (tooltipTarget && container.contains(tooltipTarget)) hideMetricTooltip();
    container.replaceChildren(...values.map(([label, value]) => metric(label, value)));
    return;
  }
  existing.forEach((item, index) => {
    const valueNode = item.querySelector<HTMLElement>('.value');
    if (!valueNode) return;
    const nextValue = String(values[index][1]);
    valueNode.textContent = nextValue;
    valueNode.dataset.fullValue = nextValue;
    if (tooltipTarget === valueNode) metricTooltip.textContent = nextValue;
  });
}

let tooltipTarget: HTMLElement | null = null;
let tooltipDismissTimer: number | null = null;

function hideMetricTooltip(): void {
  if (tooltipDismissTimer !== null) window.clearTimeout(tooltipDismissTimer);
  tooltipDismissTimer = null;
  tooltipTarget?.removeAttribute('aria-describedby');
  tooltipTarget?.removeAttribute('data-overflowing');
  tooltipTarget = null;
  metricTooltip.hidden = true;
}

function showMetricTooltip(target: HTMLElement, dismissAfterMs?: number): void {
  const overflowing = target.scrollWidth > target.clientWidth + 1;
  target.dataset.overflowing = String(overflowing);
  if (!overflowing) {
    if (tooltipTarget === target) hideMetricTooltip();
    return;
  }
  if (tooltipDismissTimer !== null) window.clearTimeout(tooltipDismissTimer);
  tooltipTarget?.removeAttribute('aria-describedby');
  tooltipTarget = target;
  target.setAttribute('aria-describedby', 'metricTooltip');
  metricTooltip.textContent = target.dataset.fullValue || target.textContent || '';
  metricTooltip.hidden = false;
  const targetRect = target.getBoundingClientRect();
  const tooltipRect = metricTooltip.getBoundingClientRect();
  const left = Math.min(window.innerWidth - tooltipRect.width - 12, Math.max(12, targetRect.left));
  const above = targetRect.top - tooltipRect.height - 8;
  const top = above >= 8 ? above : Math.max(8, Math.min(window.innerHeight - tooltipRect.height - 8, targetRect.bottom + 8));
  metricTooltip.style.left = `${left}px`;
  metricTooltip.style.top = `${top}px`;
  if (dismissAfterMs !== undefined) tooltipDismissTimer = window.setTimeout(hideMetricTooltip, dismissAfterMs);
}

document.addEventListener('pointerover', (event) => {
  const target = event.target instanceof Element ? event.target.closest<HTMLElement>('.value') : null;
  if (target) showMetricTooltip(target);
});
document.addEventListener('pointerout', (event) => {
  if (tooltipTarget && event.target === tooltipTarget) hideMetricTooltip();
});
document.addEventListener('click', (event) => {
  const target = event.target instanceof Element ? event.target.closest<HTMLElement>('.value') : null;
  if (target) showMetricTooltip(target, 3_000);
});
document.addEventListener('keydown', (event) => { if (event.key === 'Escape') hideMetricTooltip(); });
window.addEventListener('resize', hideMetricTooltip);

function chartScale(value: number): number {
  if (value <= 0) return 1;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  return Math.ceil(value / magnitude) * magnitude;
}

function drawSeriesChart<T extends { elapsed_sec: number }>(
  target: HTMLCanvasElement,
  pointsToDraw: T[],
  series: ReadonlyArray<ChartSeries<T>>,
  yAxisLabel: string,
): void {
  const context = target.getContext('2d');
  if (!context) return;
  const width = target.clientWidth * 2 || 1000;
  target.width = width;
  target.height = CHART_HEIGHT;
  context.clearRect(0, 0, width, CHART_HEIGHT);

  const padding = { left: 66, right: 24, top: 20, bottom: 48 };
  const plotWidth = width - padding.left - padding.right;
  const plotHeight = CHART_HEIGHT - padding.top - padding.bottom;
  const dataMax = Math.max(0, ...series.flatMap(({ valueFor }) => pointsToDraw.map((point) => {
    const value = valueFor(point);
    return Number.isFinite(value) ? value : 0;
  })));
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
  const xTickCount = pointsToDraw.length ? Math.min(3, pointsToDraw.length) : 1;
  for (let index = 0; index < xTickCount; index += 1) {
    const ratio = xTickCount === 1 ? 0 : index / (xTickCount - 1);
    const pointIndex = pointsToDraw.length > 1 ? Math.round(ratio * (pointsToDraw.length - 1)) : 0;
    const x = padding.left + ratio * plotWidth;
    context.fillText(pointsToDraw[pointIndex] ? `${pointsToDraw[pointIndex].elapsed_sec.toFixed(0)}s` : '0s', x, CHART_HEIGHT - 24);
  }
  context.fillStyle = '#b9c8dc';
  context.fillText('Time (rolling 60 seconds)', padding.left + plotWidth / 2, CHART_HEIGHT - 5);
  context.save();
  context.translate(17, padding.top + plotHeight / 2);
  context.rotate(-Math.PI / 2);
  context.fillText(yAxisLabel, 0, 0);
  context.restore();

  if (pointsToDraw.length < 2) return;
  series.forEach(({ color, valueFor }) => {
    context.strokeStyle = color;
    context.lineWidth = 4;
    context.beginPath();
    pointsToDraw.forEach((point, index) => {
      const x = padding.left + index * plotWidth / (pointsToDraw.length - 1);
      const y = padding.top + plotHeight - (valueFor(point) / max) * plotHeight;
      if (index) context.lineTo(x, y);
      else context.moveTo(x, y);
    });
    context.stroke();
  });
}

function drawCharts(): void {
  drawSeriesChart(chart, points, [{ key: 'bitrate', color: '#45c2ff', valueFor: (point) => point.bitrate_mbps }], 'Encoded payload (Mbps)');
  drawSeriesChart(latencyChart, latencyPoints, latencySeries.filter(({ key }) => selectedLatencySeries.has(key)), 'Estimated latency (ms)');
}

function isLatencySeriesKey(value: string | undefined): value is LatencySeriesKey {
  return latencySeries.some(({ key }) => key === value);
}

function updateLatencySeriesControls(): void {
  latencySeriesControls.querySelectorAll<HTMLButtonElement>('[data-latency-series]').forEach((button) => {
    const key = button.dataset.latencySeries;
    if (!isLatencySeriesKey(key)) return;
    const selected = selectedLatencySeries.has(key);
    button.setAttribute('aria-pressed', String(selected));
    button.disabled = selected && selectedLatencySeries.size === 1;
  });
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

function severityClass(severity: string): string {
  const normalized = severity.toLowerCase();
  return ['error', 'warn', 'info'].includes(normalized) ? normalized : 'info';
}

function readableCategory(category: string): string {
  return category.replaceAll('_', ' ');
}

function logEntry(timestamp: string, severity: string, category: string, generation: string, message: string): HTMLLIElement {
  const item = document.createElement('li');
  item.className = `log-entry log-${severityClass(severity)}`;
  const metadata = document.createElement('div');
  metadata.className = 'log-meta';
  const time = document.createElement('time');
  time.textContent = timestamp;
  const severityNode = document.createElement('span');
  severityNode.className = 'log-severity';
  severityNode.textContent = severity.toUpperCase();
  const categoryNode = document.createElement('span');
  categoryNode.className = 'log-category';
  categoryNode.textContent = readableCategory(category);
  const generationNode = document.createElement('span');
  generationNode.className = 'log-generation';
  generationNode.textContent = generation;
  metadata.append(time, severityNode, categoryNode, generationNode);
  const messageNode = document.createElement('p');
  messageNode.className = 'log-message';
  messageNode.textContent = message;
  item.append(metadata, messageNode);
  return item;
}

function replaceLogEntries(container: HTMLDivElement, entries: HTMLLIElement[], emptyMessage: string, preserveScroll = true): void {
  const nearBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 56;
  const previousScrollTop = container.scrollTop;
  if (!entries.length) {
    const empty = document.createElement('div');
    empty.className = 'log-empty';
    empty.textContent = emptyMessage;
    container.replaceChildren(empty);
    return;
  }
  const list = document.createElement('ol');
  list.className = 'log-list';
  list.append(...entries);
  container.replaceChildren(list);
  requestAnimationFrame(() => { container.scrollTop = !preserveScroll || nearBottom ? container.scrollHeight : previousScrollTop; });
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
  if (!settingsDirty && pendingGeneration === null) cloudEnabled.checked = settings.cloud_discovery_enabled;
  if (!settingsDirty && pendingGeneration === null) localPairingRequired.checked = settings.local_pairing_code_required;
  if (!settingsDirty && pendingGeneration === null) populateSettings(settings);
  cloudEnabled.disabled = pendingGeneration !== null || portalDisconnected;
  localPairingRequired.disabled = pendingGeneration !== null || portalDisconnected;
  Object.values(settingInputs).forEach((input) => { input.disabled = pendingGeneration !== null || portalDisconnected; });
  saveSettings.disabled = pendingGeneration !== null || portalDisconnected || !settingsDirty;
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
  const codeSource = settings.pairing_code_source === 'fixed' ? 'fixed deployment code' : settings.pairing_code_source === 'cloud' ? 'Cloudflare-issued fleet code with local fallback' : 'rotating receiver code';
  pairingSecuritySource.textContent = `Code source: ${codeSource}${settings.local_pairing_code_required ? '.' : '; code enforcement is disabled for direct LAN clients.'}`;
  if (pendingGeneration !== null && snapshot.watchdog.receiver_generation >= pendingGeneration && snapshot.watchdog.receiver_state === 'ready') {
    pendingGeneration = null;
    settingsDirty = false;
    cloudEnabled.checked = settings.cloud_discovery_enabled;
    cloudEnabled.disabled = false;
    Object.values(settingInputs).forEach((input) => { input.disabled = false; });
    settingsStatus.textContent = 'Receiver restarted; settings are active.';
    watchdogStatus.textContent = 'Receiver restart completed.';
  }
  replaceMetrics(watchdog, [
    ['Lifecycle', snapshot.watchdog.receiver_state.toUpperCase()],
    ['Generation', snapshot.watchdog.receiver_generation],
    ['Receiver uptime', snapshot.watchdog.receiver_uptime_sec === null ? '--' : `${snapshot.watchdog.receiver_uptime_sec}s`],
    ['Restarts', snapshot.watchdog.restart_count],
    ['Consecutive failures', snapshot.watchdog.consecutive_failures],
    ['Next retry', snapshot.watchdog.next_retry_sec === null ? '--' : `${snapshot.watchdog.next_retry_sec}s`],
    ['Logging', snapshot.watchdog.logging_healthy ? 'HEALTHY' : 'DEGRADED'],
    ['Last failure', snapshot.watchdog.last_failure || '--'],
  ]);
  restartReceiver.disabled = pendingGeneration !== null || portalDisconnected;
  const updateBusy = snapshot.update.state === 'checking' || snapshot.update.state === 'updating';
  checkUpdate.disabled = !snapshot.update.installed || updateBusy || portalDisconnected;
  applyUpdate.disabled = !snapshot.update.installed || snapshot.update.state !== 'available' || active !== null || updateBusy || portalDisconnected;
  replaceMetrics(updateMetrics, [
    ['Release', snapshot.build.revision],
    ['Current digest', snapshot.update.current_digest?.slice(0, 19) || '--'],
    ['Available digest', snapshot.update.available_digest?.slice(0, 19) || '--'],
    ['Last checked', snapshot.update.updated_at_unix ? new Date(snapshot.update.updated_at_unix * 1_000).toLocaleString() : '--'],
    ['Update state', snapshot.update.installed ? snapshot.update.state.toUpperCase().replaceAll('_', ' ') : 'UPDATER UNAVAILABLE'],
  ]);
  updateStatus.textContent = active && snapshot.update.state === 'available'
    ? 'Update available; stop casting before applying it.'
    : (snapshot.update.message || (snapshot.update.installed ? 'Use Check for update to query Docker Hub.' : 'Run the device initializer to install the host updater.'));
  if (snapshot.watchdog.configuration_error) watchdogStatus.textContent = `Receiver blocked by configuration: ${snapshot.watchdog.configuration_error}`;
  state.textContent = snapshot.watchdog.receiver_state === 'ready' ? management.state : snapshot.watchdog.receiver_state.toUpperCase();
  state.style.color = management.state === 'STREAMING' && snapshot.watchdog.receiver_state === 'ready' ? '#35d49a' : '#90a4bd';
  stopButton.disabled = !active;

  if (active) {
    const estimatedLatency = active.estimated_latency;
    const latencyAgeMs = active.estimated_latency_age_ms;
    const latencyIsStale = estimatedLatency !== null && latencyAgeMs !== null && latencyAgeMs > 3_000;
    const latencyValue = estimatedLatency
      ? `~${Math.round(estimatedLatency.total_ms)} ms${latencyIsStale ? ` (last sample ${Math.max(3, Math.floor(latencyAgeMs / 1_000))}s ago)` : ''}`
      : 'Measuring…';
    latencyFreshness.textContent = latencyIsStale
      ? `Sampling interrupted · last actual latency sample ${Math.max(3, Math.floor(latencyAgeMs / 1_000))}s ago`
      : (estimatedLatency ? 'Latency samples are current' : 'Waiting for the first latency sample');
    replaceMetrics(metrics, [
      ['Codec', active.config.codec],
      ['Resolution', active.config.encoded_resolution || active.config.resolution],
      ['Receiver access units/s (1s)', `${active.measured_fps.toFixed(1)} / ${active.config.fps} target`],
      ['Current payload throughput (1s)', `${active.measured_bitrate_mbps.toFixed(2)} Mbps`],
      ['Session avg payload throughput', `${active.average_bitrate_mbps.toFixed(2)} Mbps`],
      ['Peak payload throughput (~1s)', `${active.peak_bitrate_mbps.toFixed(2)} Mbps`],
      ['Access units submitted', active.frames.toLocaleString()],
      ['Encoded payload submitted', formatBytes(active.bytes)],
    ]);
    replaceMetrics(latencyMetrics, [
      ['Estimated total latency', latencyValue],
      ['Encoding latency', estimatedLatency ? `${Math.round(estimatedLatency.encode_ms)} ms` : 'Measuring…'],
      ['Sender queue', estimatedLatency ? `${Math.round(estimatedLatency.sender_queue_ms)} ms` : 'Measuring…'],
      ['Network delivery', estimatedLatency ? `${Math.round(estimatedLatency.delivery_ms)} ms` : 'Measuring…'],
      ['Receiver queue/input', estimatedLatency ? `${Math.round(estimatedLatency.receiver_queue_ms)} ms` : 'Measuring…'],
      ['Decode/display estimate', estimatedLatency ? `~${Math.round(estimatedLatency.decode_display_ms)} ms` : 'Measuring…'],
    ]);
    replaceMetrics(congestionMetrics, [
      ['Media write backpressure', estimatedLatency ? `${estimatedLatency.media_write_blocked_ms.toFixed(1)} ms` : 'Measuring…'],
      ['Adaptive bitrate', estimatedLatency ? `${estimatedLatency.adaptive_bitrate_mbps.toFixed(1)} / ${estimatedLatency.configured_bitrate_mbps.toFixed(1)} Mbps ceiling` : 'Measuring…'],
      ['Dropped raw input frames', estimatedLatency ? estimatedLatency.dropped_input_frames.toLocaleString() : 'Measuring…'],
      ['Effective sender FPS', estimatedLatency ? estimatedLatency.effective_fps.toFixed(1) : 'Measuring…'],
    ]);
    replaceMetrics(measurementMetrics, [
      ['Access-unit size', estimatedLatency ? formatBytes(estimatedLatency.access_unit_bytes) : 'Measuring…'],
      ['Avg first packet → playback queue', `${active.server_latency_ms.toFixed(1)} ms`],
      ['Missing sequence IDs', active.sequence_gaps],
      ['Clock confidence', estimatedLatency ? `±${estimatedLatency.clock_uncertainty_ms.toFixed(1)} ms · sync ${Math.round(estimatedLatency.clock_sync_age_ms)} ms ago` : 'Measuring…'],
    ]);
    const activeSender = active.sender;
    sender.textContent = activeSender
      ? `Active sender: ${activeSender.device_id} · ${activeSender.remote_ip} · ${activeSender.platform} · ${activeSender.user_agent}`
      : 'Active sender metadata pending';
    const samples = active.samples || [];
    const latencySamples = active.latency_samples || [];
    const latestElapsed = Math.max(
      active.duration_sec,
      samples.at(-1)?.elapsed_sec ?? 0,
      latencySamples.at(-1)?.elapsed_sec ?? 0,
    );
    if (chartSessionId !== active.id || latestElapsed < chartOriginElapsed) {
      chartSessionId = active.id;
      chartOriginElapsed = Math.max(0, latestElapsed - CHART_WINDOW_SEC);
    }
    const windowStart = Math.max(chartOriginElapsed, latestElapsed - CHART_WINDOW_SEC);
    points = samples.filter((sample) => sample.elapsed_sec >= windowStart && sample.elapsed_sec <= latestElapsed).slice(-90);
    latencyPoints = latencySamples.filter((sample) => sample.elapsed_sec >= windowStart && sample.elapsed_sec <= latestElapsed).slice(-90);
  } else {
    replaceMetrics(metrics, [
      ['Stream', 'Idle'],
    ]);
    replaceMetrics(latencyMetrics, [
      ['Estimated total latency', '--'],
      ['Encoding latency', '--'],
      ['Sender queue', '--'],
      ['Network delivery', '--'],
      ['Receiver queue/input', '--'],
      ['Decode/display estimate', '--'],
    ]);
    replaceMetrics(congestionMetrics, [
      ['Media write backpressure', '--'],
      ['Adaptive bitrate', '--'],
      ['Dropped raw input frames', '--'],
      ['Effective sender FPS', '--'],
    ]);
    replaceMetrics(measurementMetrics, [
      ['Access-unit size', '--'],
      ['Avg first packet → playback queue', '--'],
      ['Missing sequence IDs', '--'],
      ['Clock confidence', '--'],
    ]);
    sender.textContent = 'No active sender';
    latencyFreshness.textContent = 'Latency is available while streaming';
    points = [];
    latencyPoints = [];
    chartSessionId = null;
    chartOriginElapsed = 0;
  }
  drawCharts();
  renderConnections(management.connections);
  renderHistory(management.history);

  const receiverHealth = management.health;
  replaceMetrics(health, [
    ['Display', `${receiverHealth.display_resolution} @ ${receiverHealth.display_fps} Hz`],
    ['Panel', receiverHealth.panel_resolution || '--'],
    ['EDID', receiverHealth.edid_name || '--'],
    ['Pairing', snapshot.pairing.local_status],
    ['Cloud', snapshot.pairing.cloud_status],
    ['Playback pipeline', formatPlaybackState(receiverHealth.playback_state)],
    ['Reassembly in flight', receiverHealth.reassembly_in_flight ?? '--'],
    ['Dropped access units (receiver uptime)', receiverHealth.dropped_access_units ?? '--'],
    ['Ignored media packets (receiver uptime)', receiverHealth.ignored_media_packets ?? '--'],
    ['System load', formatLoad(receiverHealth)],
    ['Memory available', formatMemory(receiverHealth)],
    ['SoC temperature', typeof receiverHealth.soc_temperature_c === 'number' ? `${receiverHealth.soc_temperature_c.toFixed(1)} °C` : '--'],
  ]);
  replaceLogEntries(events, management.events.slice(-80).map((event) => logEntry(
    `${event.elapsed_sec.toFixed(1)}s`, event.level, event.kind, `g${snapshot.watchdog.receiver_generation}`, event.message,
  )), 'No receiver events');
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
      pendingGeneration = payload && typeof payload === 'object' && 'target_generation' in payload && typeof payload.target_generation === 'number' ? payload.target_generation : lastSnapshot.watchdog.receiver_generation + 1;
      settingsStatus.textContent = `Receiver restarting; waiting for generation ${pendingGeneration}…`;
    } else {
      settingsDirty = false; settingsStatus.textContent = 'Settings already active.';
    }
  } catch (error) {
    saveSettings.disabled = false; cloudEnabled.disabled = false; localPairingRequired.disabled = false;
    Object.values(settingInputs).forEach((input) => { input.disabled = false; });
    settingsStatus.textContent = error instanceof Error ? error.message : 'Settings update failed.';
  }
}

function selectTab(name: 'overview' | 'logs' | 'settings'): void {
  const tabs = [
    { name: 'overview', panel: overviewTab, button: overviewTabButton },
    { name: 'logs', panel: logsTab, button: logsTabButton },
    { name: 'settings', panel: settingsTab, button: settingsTabButton },
  ] as const;
  tabs.forEach((tab) => {
    const selected = tab.name === name;
    tab.panel.hidden = !selected;
    tab.button.classList.toggle('active', selected);
    tab.button.setAttribute('aria-selected', String(selected));
    tab.button.tabIndex = selected ? 0 : -1;
  });
  if (name === 'logs') {
    void refreshOperationalLogs();
    requestAnimationFrame(() => { operationalLogs.scrollTop = operationalLogs.scrollHeight; });
  }
}

function isSnapshot(value: unknown): value is Snapshot {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Partial<Snapshot>;
  return Boolean(candidate.management && candidate.pairing && candidate.settings && candidate.watchdog && candidate.build && candidate.update && Array.isArray(candidate.management.connections));
}

function resetChart(): void {
  const active = lastSnapshot?.management.active_stream;
  if (active) {
    const latestElapsed = Math.max(
      active.duration_sec,
      active.samples.at(-1)?.elapsed_sec ?? 0,
      active.latency_samples.at(-1)?.elapsed_sec ?? 0,
    );
    chartSessionId = active.id;
    chartOriginElapsed = latestElapsed;
  } else {
    chartSessionId = null;
    chartOriginElapsed = 0;
  }
  points = [];
  latencyPoints = [];
  drawCharts();
}

async function stopSharing(): Promise<void> {
  if (!confirm('Stop the active share?')) return;
  const response = await fetch('/api/stream/stop', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
  if (!response.ok) state.textContent = `STOP FAILED (${response.status})`;
}

async function restartReceiverNow(): Promise<void> {
  if (!lastSnapshot || !confirm('Restart the receiver? Active sharing will stop, but this portal will remain connected.')) return;
  restartReceiver.disabled = true;
  watchdogStatus.textContent = 'Requesting receiver restart…';
  const response = await fetch('/api/watchdog/restart', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ confirm_restart: true }) });
  const payload: unknown = await response.json().catch(() => ({}));
  if (!response.ok || !payload || typeof payload !== 'object' || !('target_generation' in payload) || typeof payload.target_generation !== 'number') {
    restartReceiver.disabled = false; watchdogStatus.textContent = `Restart failed (${response.status}).`; return;
  }
  pendingGeneration = payload.target_generation;
  watchdogStatus.textContent = `Receiver restarting; waiting for generation ${pendingGeneration}…`;
}

async function requestUpdate(path: 'check' | 'apply'): Promise<void> {
  if (path === 'apply' && (!lastSnapshot || lastSnapshot.management.active_stream)) return;
  if (path === 'apply' && !confirm('Install the available update? The management portal will reconnect after the container restarts.')) return;
  checkUpdate.disabled = true;
  applyUpdate.disabled = true;
  updateStatus.textContent = path === 'check' ? 'Checking Docker Hub…' : 'Installing update; waiting for the device to restart…';
  const response = await fetch(`/api/update/${path}`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: path === 'apply' ? JSON.stringify({ confirm_update: true }) : '{}',
  });
  if (!response.ok) {
    const payload: unknown = await response.json().catch(() => ({}));
    const reason = payload && typeof payload === 'object' && 'error' in payload && typeof payload.error === 'string' ? payload.error.replaceAll('_', ' ') : `HTTP ${response.status}`;
    updateStatus.textContent = `Update request failed: ${reason}.`;
    if (lastSnapshot) render(lastSnapshot);
  }
}

function renderOperationalLogs(): void {
  const severity = logSeverity.value;
  const category = logCategory.value;
  const query = logSearch.value.trim().toLocaleLowerCase();
  const filtered = operationalEvents.filter((event) =>
    (severity === 'all' || event.severity === severity)
    && (category === 'all' || event.category === category)
    && (!query || `${event.category} ${event.message}`.toLocaleLowerCase().includes(query))
  );
  replaceLogEntries(operationalLogs, filtered.map((event) => logEntry(
    new Date(event.timestamp_unix_ms).toLocaleString(), event.severity, event.category,
    `g${event.receiver_generation}`, event.message,
  )), 'No operational events match these filters');
  logCount.textContent = `Showing ${filtered.length} of ${operationalEvents.length} retained events`;
}

async function refreshOperationalLogs(): Promise<void> {
  const response = await fetch('/api/logs?lines=300');
  if (!response.ok) return;
  const payload: unknown = await response.json().catch(() => ({}));
  if (payload && typeof payload === 'object' && 'events' in payload && Array.isArray(payload.events)) {
    operationalEvents = payload.events as OperationalEvent[];
    const selectedCategory = logCategory.value;
    const categories = [...new Set(operationalEvents.map((event) => event.category))].sort();
    logCategory.replaceChildren(new Option('All categories', 'all'), ...categories.map((category) => new Option(readableCategory(category), category)));
    logCategory.value = categories.includes(selectedCategory) ? selectedCategory : 'all';
    renderOperationalLogs();
  }
}

async function downloadDiagnosticZip(): Promise<void> {
  downloadLogs.disabled = true;
  try {
    const response = await fetch('/api/logs/download');
    if (!response.ok) throw new Error(`download failed (${response.status})`);
    const url = URL.createObjectURL(await response.blob());
    const link = document.createElement('a'); link.href = url; link.download = 'llrdc-diagnostics.zip'; link.click(); URL.revokeObjectURL(url);
  } catch (error) { watchdogStatus.textContent = error instanceof Error ? error.message : 'Diagnostic download failed.'; }
  finally { downloadLogs.disabled = false; }
}

function connect(): void {
  const protocol = location.protocol === 'https:' ? 'wss' : 'ws';
  const socket = new WebSocket(`${protocol}://${location.host}/ws`);
  socket.addEventListener('open', () => { portalDisconnected = false; });
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
    document.documentElement.dataset.portalDisconnects = String(Number(document.documentElement.dataset.portalDisconnects || '0') + 1);
    portalDisconnected = true;
    if (pendingGeneration !== null) settingsStatus.textContent = 'Management connection interrupted; reconnecting…';
    state.textContent = 'DISCONNECTED';
    window.setTimeout(connect, 1500);
  });
  socket.addEventListener('error', () => socket.close());
}

resetChartButton.addEventListener('click', resetChart);
latencySeriesControls.addEventListener('click', (event) => {
  const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>('[data-latency-series]') : null;
  const key = button?.dataset.latencySeries;
  if (!isLatencySeriesKey(key)) return;
  if (selectedLatencySeries.has(key)) {
    if (selectedLatencySeries.size === 1) return;
    selectedLatencySeries.delete(key);
  } else {
    selectedLatencySeries.add(key);
  }
  updateLatencySeriesControls();
  drawCharts();
});
stopButton.addEventListener('click', () => void stopSharing());
restartReceiver.addEventListener('click', () => void restartReceiverNow());
checkUpdate.addEventListener('click', () => void requestUpdate('check'));
applyUpdate.addEventListener('click', () => void requestUpdate('apply'));
downloadLogs.addEventListener('click', () => void downloadDiagnosticZip());
logSeverity.addEventListener('change', renderOperationalLogs);
logCategory.addEventListener('change', renderOperationalLogs);
logSearch.addEventListener('input', renderOperationalLogs);
jumpLatest.addEventListener('click', () => { operationalLogs.scrollTo({ top: operationalLogs.scrollHeight, behavior: 'smooth' }); });
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
logsTabButton.addEventListener('click', () => selectTab('logs'));
settingsTabButton.addEventListener('click', () => selectTab('settings'));
[overviewTabButton, logsTabButton, settingsTabButton].forEach((button, index, buttons) => button.addEventListener('keydown', (event) => {
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
  event.preventDefault();
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1
    : (index + (event.key === 'ArrowRight' ? 1 : -1) + buttons.length) % buttons.length;
  buttons[next].click();
  buttons[next].focus();
}));
window.addEventListener('resize', drawCharts);
updateLatencySeriesControls();
connect();
void refreshOperationalLogs();
window.setInterval(() => void refreshOperationalLogs(), 5000);
