export interface EncoderTiming {
  captureTimeMs: number;
  encodeDurationMs: number;
}

export interface LatencyComponents {
  totalMs: number;
  encodeMs: number;
  transportQueueMs: number;
  decodeDisplayMs: number;
}

const MAX_TRACKED_FRAMES = 64;
const MAX_SAMPLE_AGE_MS = 5_000;
const MAX_ENCODE_DURATION_MS = 2_000;
const MIN_DISPLAY_FPS = 1;
const MAX_DISPLAY_FPS = 240;

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
    this.trackedFrames += 1;
    while (this.trackedFrames > MAX_TRACKED_FRAMES) {
      const oldest = this.captures.entries().next().value as [number, number[]] | undefined;
      if (!oldest) break;
      oldest[1].shift();
      this.trackedFrames -= 1;
      if (oldest[1].length === 0) this.captures.delete(oldest[0]);
    }
  }

  resolve(timestamp: number, encodedAtMs = monotonicEpochMs()): EncoderTiming | null {
    const entries = this.captures.get(timestamp);
    const captureTimeMs = entries?.shift();
    if (captureTimeMs === undefined) return null;
    this.trackedFrames -= 1;
    if (entries?.length === 0) this.captures.delete(timestamp);
    return {
      captureTimeMs,
      encodeDurationMs: Math.max(0, encodedAtMs - captureTimeMs),
    };
  }

  reset(): void {
    this.captures.clear();
    this.trackedFrames = 0;
  }
}

export function calculateLatencyComponents(
  captureTimeMs: number,
  encodeDurationMs: number,
  receivedAtMs: number,
  rttMs: number,
  displayFps: number,
): LatencyComponents | null {
  if (![captureTimeMs, encodeDurationMs, receivedAtMs, rttMs, displayFps].every(Number.isFinite)) return null;
  const ageMs = receivedAtMs - captureTimeMs;
  if (ageMs < 0 || ageMs > MAX_SAMPLE_AGE_MS) return null;
  if (encodeDurationMs < 0 || encodeDurationMs > MAX_ENCODE_DURATION_MS || encodeDurationMs > ageMs) return null;
  if (rttMs < 0 || rttMs > MAX_SAMPLE_AGE_MS) return null;
  if (displayFps < MIN_DISPLAY_FPS || displayFps > MAX_DISPLAY_FPS) return null;

  const measuredPipelineMs = Math.max(encodeDurationMs, ageMs - rttMs / 2);
  const transportQueueMs = Math.max(0, measuredPipelineMs - encodeDurationMs);
  const decodeDisplayMs = 1_000 / displayFps;
  return {
    totalMs: encodeDurationMs + transportQueueMs + decodeDisplayMs,
    encodeMs: encodeDurationMs,
    transportQueueMs,
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
      return { ...this.value };
    }
    const blend = (previous: number, next: number): number => previous + this.alpha * (next - previous);
    this.value = {
      totalMs: blend(this.value.totalMs, sample.totalMs),
      encodeMs: blend(this.value.encodeMs, sample.encodeMs),
      transportQueueMs: blend(this.value.transportQueueMs, sample.transportQueueMs),
      decodeDisplayMs: blend(this.value.decodeDisplayMs, sample.decodeDisplayMs),
    };
    return { ...this.value };
  }

  reset(): void {
    this.value = null;
  }
}
