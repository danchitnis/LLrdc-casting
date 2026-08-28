import assert from 'node:assert/strict';
import { calculateLatencyComponents, EncoderTimingTracker, LatencySmoother } from './latency.ts';

{
  const tracker = new EncoderTimingTracker();
  tracker.mark(10, 1_000);
  tracker.mark(10, 1_005);
  assert.deepEqual(tracker.resolve(10, 1_012), { captureTimeMs: 1_000, encodeDurationMs: 12 });
  assert.deepEqual(tracker.resolve(10, 1_020), { captureTimeMs: 1_005, encodeDurationMs: 15 });
  assert.equal(tracker.resolve(10, 1_030), null);
  tracker.mark(20, 2_000);
  tracker.reset();
  assert.equal(tracker.resolve(20, 2_010), null);
}

{
  const sample = calculateLatencyComponents(1_000, 15, 1_060, 20, 60);
  assert.ok(sample);
  assert.equal(sample.encodeMs, 15);
  assert.equal(sample.transportQueueMs, 35);
  assert.ok(Math.abs(sample.decodeDisplayMs - 16.6667) < 0.001);
  assert.ok(Math.abs(sample.totalMs - 66.6667) < 0.001);
}

assert.equal(calculateLatencyComponents(1_100, 5, 1_000, 10, 60), null, 'future capture timestamps are rejected');
assert.equal(calculateLatencyComponents(1_000, 70, 1_060, 10, 60), null, 'encode time cannot exceed sample age');
assert.equal(calculateLatencyComponents(1_000, 5, 7_000, 10, 60), null, 'stale samples are rejected');
assert.equal(calculateLatencyComponents(1_000, 5, 1_020, 10, 0), null, 'invalid display rates are rejected');

{
  const smoother = new LatencySmoother(0.25);
  const first = { totalMs: 40, encodeMs: 10, transportQueueMs: 13, decodeDisplayMs: 17 };
  assert.deepEqual(smoother.update(first), first);
  assert.deepEqual(smoother.update({ totalMs: 80, encodeMs: 30, transportQueueMs: 33, decodeDisplayMs: 17 }), {
    totalMs: 50,
    encodeMs: 15,
    transportQueueMs: 18,
    decodeDisplayMs: 17,
  });
  smoother.reset();
  assert.deepEqual(smoother.update(first), first);
}

console.log('latency tests passed');
