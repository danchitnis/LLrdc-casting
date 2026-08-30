export interface EncoderTiming {
  captureTimeMs: number;
  encodeDurationMs: number;
}

export interface LatencyComponents {
  totalMs: number;
  encodeMs: number;
  senderQueueMs: number;
  deliveryMs: number;
  receiverQueueMs: number;
  decodeDisplayMs: number;
}

export interface ClockEstimate {
  offsetMs: number;
  uncertaintyMs: number;
  sampledAtMs: number;
}

const MAX_TRACKED_FRAMES = 64;
const MAX_SAMPLE_AGE_MS = 30_000;
const MAX_PIPELINE_DURATION_MS = 30_000;
const MIN_REPORT_INTERVAL_MS = 1_000;
const CLOCK_SAMPLE_LIMIT = 8;
const CLOCK_DRIFT_MS_PER_MS = 0.00005;

export function monotonicEpochMs(): number {
  return performance.timeOrigin + performance.now();
}

export class EncoderTimingTracker {
  private readonly captures = new Map<number, number[]>();
  private trackedFrames = 0;

  mark(timestamp: number, captureTimeMs = monotonicEpochMs()): void {
    const entries = this.captures.get(timestamp) ?? [];
    entries.push(captureTimeMs);
    this.captures.set(timestamp, entries);
    this.trackedFrames++;
    while (this.trackedFrames > MAX_TRACKED_FRAMES) {
      const oldest = this.captures.entries().next().value as [number, number[]] | undefined;
      if (!oldest) break;
      oldest[1].shift();
      this.trackedFrames--;
      if (!oldest[1].length) this.captures.delete(oldest[0]);
    }
  }

  resolve(timestamp: number, encodedAtMs = monotonicEpochMs()): EncoderTiming | null {
    const entries = this.captures.get(timestamp);
    const captureTimeMs = entries?.shift();
    if (captureTimeMs === undefined) return null;
    this.trackedFrames--;
    if (!entries?.length) this.captures.delete(timestamp);
    return { captureTimeMs, encodeDurationMs: Math.max(0, encodedAtMs - captureTimeMs) };
  }

  reset(): void {
    this.captures.clear();
    this.trackedFrames = 0;
  }
}

export class ClockSynchronizer {
  private samples: Array<ClockEstimate & { delayMs: number }> = [];

  record(t0: number, s1: number, s2: number, t3: number): ClockEstimate | null {
    if (![t0, s1, s2, t3].every(Number.isFinite)) return null;
    const serverWorkMs = s2 - s1;
    const delayMs = t3 - t0 - serverWorkMs;
    if (serverWorkMs < 0 || delayMs < 0 || delayMs > MAX_SAMPLE_AGE_MS) return null;
    this.samples.push({
      offsetMs: ((s1 - t0) + (s2 - t3)) / 2,
      uncertaintyMs: delayMs / 2,
      sampledAtMs: t3,
      delayMs,
    });
    if (this.samples.length > CLOCK_SAMPLE_LIMIT) this.samples.shift();
    return this.estimate(t3);
  }

  estimate(nowMs: number): ClockEstimate | null {
    if (!Number.isFinite(nowMs) || !this.samples.length) return null;
    const best = this.samples.reduce((selected, sample) => sample.delayMs < selected.delayMs ? sample : selected);
    const ageMs = nowMs - best.sampledAtMs;
    if (ageMs < 0) return null;
    return {
      offsetMs: best.offsetMs,
      uncertaintyMs: best.uncertaintyMs + ageMs * CLOCK_DRIFT_MS_PER_MS,
      sampledAtMs: best.sampledAtMs,
    };
  }

  reset(): void {
    this.samples = [];
  }
}

export interface PhaseTimingSample {
  captureTimeMs: number;
  encodeDurationMs: number;
  sendStartTimeMs: number;
  receiverCompleteTimeMs: number;
  receiverQueueMs: number;
}

export function calculateLatencyComponents(
  sample: PhaseTimingSample,
  clock: ClockEstimate,
  displayFps: number,
): LatencyComponents | null {
  if (![...Object.values(sample), clock.offsetMs, clock.uncertaintyMs, displayFps].every(Number.isFinite)) return null;
  if (sample.encodeDurationMs < 0 || sample.encodeDurationMs > MAX_PIPELINE_DURATION_MS
    || sample.receiverQueueMs < 0 || displayFps < 1 || displayFps > 240 || clock.uncertaintyMs < 0) return null;

  const senderQueueRawMs = sample.sendStartTimeMs - sample.captureTimeMs - sample.encodeDurationMs;
  const deliveryRawMs = sample.receiverCompleteTimeMs - clock.offsetMs - sample.sendStartTimeMs;
  if (senderQueueRawMs < -clock.uncertaintyMs || deliveryRawMs < -clock.uncertaintyMs) return null;

  const senderQueueMs = Math.max(0, senderQueueRawMs);
  const deliveryMs = Math.max(0, deliveryRawMs);
  const pipelineMs = sample.encodeDurationMs + senderQueueMs + deliveryMs + sample.receiverQueueMs;
  if (pipelineMs > MAX_PIPELINE_DURATION_MS) return null;
  const decodeDisplayMs = 1_000 / displayFps;
  return {
    totalMs: pipelineMs + decodeDisplayMs,
    encodeMs: sample.encodeDurationMs,
    senderQueueMs,
    deliveryMs,
    receiverQueueMs: sample.receiverQueueMs,
    decodeDisplayMs,
  };
}

export class LatencySmoother {
  private value: LatencyComponents | null = null;
  private readonly alpha: number;

  constructor(alpha = 0.25) {
    this.alpha = alpha;
  }

  update(sample: LatencyComponents): LatencyComponents {
    if (!this.value) {
      this.value = { ...sample };
      return { ...sample };
    }
    const blend = (previous: number, next: number): number => previous + this.alpha * (next - previous);
    this.value = {
      totalMs: blend(this.value.totalMs, sample.totalMs),
      encodeMs: blend(this.value.encodeMs, sample.encodeMs),
      senderQueueMs: blend(this.value.senderQueueMs, sample.senderQueueMs),
      deliveryMs: blend(this.value.deliveryMs, sample.deliveryMs),
      receiverQueueMs: blend(this.value.receiverQueueMs, sample.receiverQueueMs),
      decodeDisplayMs: blend(this.value.decodeDisplayMs, sample.decodeDisplayMs),
    };
    return { ...this.value };
  }

  reset(): void {
    this.value = null;
  }
}

export interface PendingLatencySample extends PhaseTimingSample {
  seq: number;
  accessUnitBytes?: number;
  writeBlockedMs?: number;
  droppedInputFrames?: number;
  configuredBitrateMbps?: number;
  adaptiveBitrateMbps?: number;
  effectiveFps?: number;
}

export interface PreparedLatencySample {
  seq: number;
  components: LatencyComponents;
  clockUncertaintyMs: number;
  clockAgeMs: number;
  diagnostics: Omit<PendingLatencySample, keyof PhaseTimingSample | 'seq'>;
}

export class LatencySampleCoordinator {
  private pending: PendingLatencySample | null = null;
  private lastReportAtMs: number | null = null;
  readonly clock = new ClockSynchronizer();

  acknowledge(sample: PendingLatencySample): void {
    if (sample.seq > 0 && Object.values(sample).every(value => value === undefined || Number.isFinite(value))) {
      this.pending = sample;
    }
  }

  prepare(nowMs: number, displayFps: number): PreparedLatencySample | null {
    const estimate = this.clock.estimate(nowMs);
    if (!this.pending || !estimate || !Number.isFinite(nowMs)) return null;
    if (this.lastReportAtMs !== null && nowMs - this.lastReportAtMs < MIN_REPORT_INTERVAL_MS) return null;

    const pending = this.pending;
    this.pending = null;
    const receiverCompleteClientMs = pending.receiverCompleteTimeMs - estimate.offsetMs;
    if (pending.captureTimeMs > nowMs + estimate.uncertaintyMs
      || receiverCompleteClientMs > nowMs + estimate.uncertaintyMs
      || nowMs - pending.captureTimeMs > MAX_SAMPLE_AGE_MS) return null;
    const components = calculateLatencyComponents(pending, estimate, displayFps);
    if (!components) return null;

    this.lastReportAtMs = nowMs;
    const {
      seq,
      captureTimeMs: _capture,
      encodeDurationMs: _encode,
      sendStartTimeMs: _send,
      receiverCompleteTimeMs: _receiver,
      receiverQueueMs: _queue,
      ...diagnostics
    } = pending;
    return {
      seq,
      components,
      clockUncertaintyMs: estimate.uncertaintyMs,
      clockAgeMs: nowMs - estimate.sampledAtMs,
      diagnostics,
    };
  }

  reset(): void {
    this.pending = null;
    this.lastReportAtMs = null;
    this.clock.reset();
  }
}
