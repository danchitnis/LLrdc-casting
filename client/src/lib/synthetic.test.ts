import { formatSyntheticStatus, type SyntheticStreamConfig } from './synthetic.ts';

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

const config: SyntheticStreamConfig = {
  width: 1280,
  height: 720,
  encodedWidth: 1280,
  encodedHeight: 720,
  fps: 60,
  codec: 'H264',
  hardwarePreference: 'prefer-software',
  bitrate: 8_000_000,
  aspectMode: 'stretch',
  latencyMode: 'balanced',
};

const status = formatSyntheticStatus(config, 120);
assert(status[0] === 'H.264 (SW / CPU)', 'status includes codec and acceleration mode');
assert(status[1].includes('Source: 1280x720') && status[1].includes('Coded: 1280x720'), 'status includes source and coded dimensions');
assert(status[2].includes('60 FPS') && status[2].includes('8.0 Mbps') && status[2].includes('Aspect: stretch'), 'status includes output settings');
assert(status[3] === 'Priority: balanced', 'status includes encoding priority');
assert(status[4].includes('Frame #120') && status[4].includes('2.00s'), 'status includes frame timing');

console.log('synthetic configuration tests passed');
