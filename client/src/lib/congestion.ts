export type CongestionMode = 'ULL' | 'balanced' | 'quality';
export interface CongestionSnapshot { currentBitrate: number; droppedInputFrames: number; pendingWrites: number; congested: boolean; bitrateChanged: boolean; }
interface WindowSample { atMs: number; senderQueueMs: number; writeBlockedMs: number; }
interface InputSample { atMs: number; limited: boolean; }

export class CongestionController {
  private samples: WindowSample[] = [];
  private inputSamples: InputSample[] = [];
  private currentBitrate: number;
  private dropped = 0;
  private pendingWrites = 0;
  private congestedWindows = 0;
  private healthySinceMs: number | null = null;
  private lastChangeMs = -Infinity;
  private lastEvaluationMs = -Infinity;
  private readonly mode: CongestionMode;
  private readonly ceilingBitrate: number;
  private readonly frameIntervalMs: number;
  constructor(mode: CongestionMode, ceilingBitrate: number, frameIntervalMs: number) {
    this.mode = mode; this.ceilingBitrate = ceilingBitrate; this.frameIntervalMs = frameIntervalMs; this.currentBitrate = ceilingBitrate;
  }
  get encoderQueueLimit(): number { return this.mode === 'ULL' ? 1 : this.mode === 'balanced' ? 2 : 8; }
  get writeLimit(): number { return this.mode === 'ULL' ? 1 : this.mode === 'balanced' ? 2 : 8; }
  shouldDropInput(encoderQueueSize: number, atMs = performance.now()): boolean {
    const drop = encoderQueueSize >= this.encoderQueueLimit || this.pendingWrites >= this.writeLimit;
    this.inputSamples.push({ atMs, limited: drop });
    this.inputSamples = this.inputSamples.filter(sample => atMs - sample.atMs <= 2_000);
    if (drop) this.dropped++;
    return drop;
  }
  writeStarted(): void { this.pendingWrites++; }
  writeFinished(atMs: number, senderQueueMs: number, writeBlockedMs: number): CongestionSnapshot {
    this.pendingWrites = Math.max(0, this.pendingWrites - 1);
    this.samples.push({ atMs, senderQueueMs, writeBlockedMs });
    this.samples = this.samples.filter(sample => atMs - sample.atMs <= 2_000);
    return this.evaluate(atMs);
  }
  private evaluate(nowMs: number): CongestionSnapshot {
    if (nowMs - this.lastEvaluationMs < 2_000) return this.snapshot();
    this.lastEvaluationMs = nowMs;
    this.inputSamples = this.inputSamples.filter(sample => nowMs - sample.atMs <= 2_000);
    const sorted = (key: 'senderQueueMs' | 'writeBlockedMs') => this.samples.map(s => s[key]).sort((a, b) => a - b);
    const percentile = (values: number[]) => values.length ? values[Math.min(values.length - 1, Math.floor(values.length * 0.95))] : 0;
    const limitedRatio = this.inputSamples.length ? this.inputSamples.filter(sample => sample.limited).length / this.inputSamples.length : 0;
    const congested = percentile(sorted('senderQueueMs')) > this.frameIntervalMs || percentile(sorted('writeBlockedMs')) > this.frameIntervalMs || limitedRatio >= 0.1;
    this.congestedWindows = congested ? this.congestedWindows + 1 : 0;
    let changed = false;
    if (congested) this.healthySinceMs = null;
    else if (this.healthySinceMs === null) this.healthySinceMs = nowMs;
    if (this.congestedWindows >= 2 && nowMs - this.lastChangeMs >= 2_000) {
      const floor = Math.round(this.ceilingBitrate * 0.4), next = Math.max(floor, Math.round(this.currentBitrate * 0.8));
      if (next < this.currentBitrate) { this.currentBitrate = next; changed = true; this.lastChangeMs = nowMs; }
      this.congestedWindows = 0;
    } else if (!congested && this.healthySinceMs !== null && nowMs - this.healthySinceMs >= 10_000 && nowMs - this.lastChangeMs >= 5_000) {
      const next = Math.min(this.ceilingBitrate, this.currentBitrate + Math.round(this.ceilingBitrate * 0.1));
      if (next > this.currentBitrate) { this.currentBitrate = next; changed = true; this.lastChangeMs = nowMs; }
    }
    return { currentBitrate: this.currentBitrate, droppedInputFrames: this.dropped, pendingWrites: this.pendingWrites, congested, bitrateChanged: changed };
  }
  snapshot(): CongestionSnapshot { return { currentBitrate: this.currentBitrate, droppedInputFrames: this.dropped, pendingWrites: this.pendingWrites, congested: false, bitrateChanged: false }; }
  reset(): void { this.samples = []; this.inputSamples = []; this.currentBitrate = this.ceilingBitrate; this.dropped = 0; this.pendingWrites = 0; this.congestedWindows = 0; this.healthySinceMs = null; this.lastChangeMs = -Infinity; this.lastEvaluationMs = -Infinity; }
}
