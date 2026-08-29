import assert from 'node:assert/strict';
import { calculateLatencyComponents, EncoderTimingTracker, LatencySampleCoordinator, LatencySmoother } from './latency.ts';

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
assert.ok(calculateLatencyComponents(1_000, 5, 7_000, 10, 60), 'visible pipeline delays above five seconds are accepted');
assert.equal(calculateLatencyComponents(1_000, 5, 31_001, 10, 60), null, 'samples older than thirty seconds are rejected');
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

{
  const coordinator = new LatencySampleCoordinator();
  coordinator.acknowledge({ seq: 1, captureTimeMs: 1_000, encodeDurationMs: 5, receivedAtMs: 1_040 });
  assert.equal(coordinator.prepare(1_041, 60), null, 'acknowledgement waits for an RTT sample');
  coordinator.recordRtt(10, 1_050);
  const firstPrepared = coordinator.prepare(1_050, 60);
  assert.equal(firstPrepared?.seq, 1, 'acknowledgement is retained until pong arrives');
  assert.equal(firstPrepared?.rttAgeMs, 0);

  coordinator.acknowledge({ seq: 31, captureTimeMs: 2_000, encodeDurationMs: 5, receivedAtMs: 2_040 });
  assert.equal(coordinator.prepare(2_040, 60), null, 'reports are limited to one per second');
  assert.equal(coordinator.prepare(2_100, 60)?.seq, 31, 'a recent RTT remains usable across an isolated lost pong');

  coordinator.acknowledge({ seq: 61, captureTimeMs: 12_000, encodeDurationMs: 5, receivedAtMs: 12_040 });
  const staleRttPrepared = coordinator.prepare(12_051, 60);
  assert.equal(staleRttPrepared?.seq, 61, 'the last valid RTT remains usable while the control session is healthy');
  assert.equal(staleRttPrepared?.rttAgeMs, 11_001, 'RTT correction age remains available internally');

  coordinator.acknowledge({ seq: 91, captureTimeMs: 61_000, encodeDurationMs: 5, receivedAtMs: 61_040 });
  assert.equal(coordinator.prepare(61_050, 60)?.seq, 91, 'a validated RTT does not halt reporting after sixty seconds');
  coordinator.reset();
  coordinator.acknowledge({ seq: 121, captureTimeMs: 62_000, encodeDurationMs: 5, receivedAtMs: 62_040 });
  assert.equal(coordinator.prepare(62_100, 60), null, 'reset removes RTT and pending coordination state');
}

console.log('latency tests passed');
