import assert from 'node:assert/strict';
import { calculateLatencyComponents, ClockSynchronizer, EncoderTimingTracker, LatencySampleCoordinator, LatencySmoother } from './latency.ts';

{
  const tracker = new EncoderTimingTracker();
  tracker.mark(10, 1_000); tracker.mark(10, 1_005);
  assert.deepEqual(tracker.resolve(10, 1_012), { captureTimeMs: 1_000, encodeDurationMs: 12 });
  assert.deepEqual(tracker.resolve(10, 1_020), { captureTimeMs: 1_005, encodeDurationMs: 15 });
  assert.equal(tracker.resolve(10, 1_030), null); tracker.reset();
}

{
  const clock = new ClockSynchronizer();
  clock.record(1_000, 1_106, 1_107, 1_013);
  clock.record(2_000, 2_104, 2_105, 2_011);
  const estimate = clock.estimate(2_011);
  assert.ok(estimate); assert.equal(estimate.offsetMs, 99); assert.equal(estimate.uncertaintyMs, 5);
}

{
  const sample = calculateLatencyComponents({ captureTimeMs: 1_000, encodeDurationMs: 10, sendStartTimeMs: 1_015, receiverCompleteTimeMs: 1_125, receiverQueueMs: 5 }, { offsetMs: 100, uncertaintyMs: 2, sampledAtMs: 1_020 }, 60);
  assert.ok(sample); assert.equal(sample.encodeMs, 10); assert.equal(sample.senderQueueMs, 5);
  assert.equal(sample.deliveryMs, 10); assert.equal(sample.receiverQueueMs, 5);
  assert.ok(Math.abs(sample.totalMs - 46.6667) < 0.001);
  const slowStartup = calculateLatencyComponents({ captureTimeMs: 1_000, encodeDurationMs: 2_500, sendStartTimeMs: 3_505, receiverCompleteTimeMs: 3_610, receiverQueueMs: 5 }, { offsetMs: 100, uncertaintyMs: 2, sampledAtMs: 3_610 }, 60);
  assert.ok(slowStartup, 'hardware encoder startup above two seconds remains measurable');
  assert.equal(slowStartup.encodeMs, 2_500);
  assert.equal(calculateLatencyComponents({ captureTimeMs: 1_000, encodeDurationMs: 5, sendStartTimeMs: 990, receiverCompleteTimeMs: 1_110, receiverQueueMs: 1 }, { offsetMs: 100, uncertaintyMs: 2, sampledAtMs: 1_000 }, 60), null);
  assert.equal(calculateLatencyComponents({ captureTimeMs: 1_000, encodeDurationMs: 5, sendStartTimeMs: 1_005, receiverCompleteTimeMs: 32_000, receiverQueueMs: 1 }, { offsetMs: 0, uncertaintyMs: 2, sampledAtMs: 1_000 }, 60), null);
}

{
  const smoother = new LatencySmoother(0.25);
  const first = { totalMs: 50, encodeMs: 10, senderQueueMs: 5, deliveryMs: 10, receiverQueueMs: 8, decodeDisplayMs: 17 };
  assert.deepEqual(smoother.update(first), first);
  assert.deepEqual(smoother.update({ totalMs: 70, encodeMs: 30, senderQueueMs: 9, deliveryMs: 14, receiverQueueMs: 12, decodeDisplayMs: 17 }), { totalMs: 55, encodeMs: 15, senderQueueMs: 6, deliveryMs: 11, receiverQueueMs: 9, decodeDisplayMs: 17 });
  smoother.reset(); assert.deepEqual(smoother.update(first), first);
}

{
  const coordinator = new LatencySampleCoordinator();
  coordinator.acknowledge({ seq: 1, captureTimeMs: 1_000, encodeDurationMs: 5, sendStartTimeMs: 1_007, receiverCompleteTimeMs: 1_112, receiverQueueMs: 3 });
  assert.equal(coordinator.prepare(1_012, 60), null, 'acknowledgement waits for synchronization');
  coordinator.clock.record(1_000, 1_103, 1_104, 1_011);
  const first = coordinator.prepare(1_012, 60);
  assert.equal(first?.seq, 1); assert.ok(Math.abs((first?.clockUncertaintyMs ?? 0) - 5) < 0.001);
  coordinator.acknowledge({ seq: 2, captureTimeMs: 2_000, encodeDurationMs: 5, sendStartTimeMs: 2_007, receiverCompleteTimeMs: 2_112, receiverQueueMs: 3 });
  assert.equal(coordinator.prepare(2_000, 60), null);
  assert.equal(coordinator.prepare(2_100, 60)?.seq, 2);
  coordinator.reset(); assert.equal(coordinator.prepare(3_100, 60), null);
}

console.log('latency tests passed');
