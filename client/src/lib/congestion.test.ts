import assert from 'node:assert/strict';
import { CongestionController } from './congestion.ts';

class BlockedWriter {
  readonly written: number[] = [];
  private releases: Array<() => void> = [];

  write(sequence: number): Promise<void> {
    this.written.push(sequence);
    return new Promise(resolve => this.releases.push(resolve));
  }

  release(): void {
    this.releases.shift()?.();
  }
}

const controller = new CongestionController('ULL', 10_000_000, 16.7);
const writer = new BlockedWriter();
const encoded: number[] = [];
let lastResult = controller.snapshot();

async function selectFrame(sequence: number, atMs: number): Promise<boolean> {
  if (controller.shouldDropInput(0, atMs - 10)) return false;
  encoded.push(sequence);
  controller.writeStarted();
  const blocked = writer.write(sequence);
  for (let attempt = 0; attempt < 4; attempt++) assert.equal(controller.shouldDropInput(0, atMs - 5 + attempt), true);
  writer.release();
  await blocked;
  lastResult = controller.writeFinished(atMs, 30, 30);
  return true;
}

assert.equal(controller.encoderQueueLimit, 1);
assert.equal(await selectFrame(1, 2_000), true);
assert.equal(await selectFrame(2, 4_100), true);
assert.equal(lastResult.currentBitrate, 8_000_000);
assert.equal(lastResult.bitrateChanged, true);
assert.deepEqual(writer.written, encoded, 'every encoded access unit reaches the reliable writer in order');
assert.ok(lastResult.droppedInputFrames >= 8, 'blocked transport drops newly captured raw frames');

for (let second = 5; second <= 20; second++) {
  controller.writeStarted();
  lastResult = controller.writeFinished(second * 1_000, 1, 1);
}
assert.ok(lastResult.currentBitrate > 8_000_000, 'bitrate recovers after a healthy interval');
controller.reset();
assert.equal(controller.snapshot().currentBitrate, 10_000_000);
assert.equal(controller.snapshot().pendingWrites, 0);

console.log('congestion tests passed');
