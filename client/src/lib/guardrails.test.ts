import {
  alignEncoderDimensions,
  getCodecString,
  isCodecResolutionAllowed,
} from './guardrails.ts';

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

const h2641080p = alignEncoderDimensions('H264', 1920, 1080);
assert(h2641080p.width === 1920 && h2641080p.height === 1088, 'H.264 1080p must use a 1920x1088 coded surface');
assert(isCodecResolutionAllowed('H264', h2641080p), 'H.264 1920x1088 must be allowed');
assert(!isCodecResolutionAllowed('H264', { width: 2560, height: 1440 }), 'H.264 1440p must be rejected');

const hevc1080p = alignEncoderDimensions('H265', 1920, 1080);
assert(hevc1080p.width === 1920 && hevc1080p.height === 1088, 'HEVC 1080p must use a 1920x1088 coded surface');
assert(getCodecString('H264', 1920, 30) === 'avc1.42e028', 'H.264 30 FPS must use level 4.0');
assert(getCodecString('H264', 1920, 60) === 'avc1.42e02a', 'H.264 60 FPS must use level 4.2');

console.log('codec guardrail tests passed');
