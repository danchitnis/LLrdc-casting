import dgram from 'node:dgram';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execSync } from 'node:child_process';
import ffmpegPath from 'ffmpeg-static';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const SERVER_HOST = process.argv[2] || process.env.BOARD_IP || '192.168.1.72';
const SERVER_PORT = parseInt(process.argv[3] || process.env.BOARD_PORT || '4434', 10);
const VID_W = parseInt(process.argv[4] || process.env.BOARD_WIDTH || '1280', 10);
const VID_H = parseInt(process.argv[5] || process.env.BOARD_HEIGHT || '720', 10);

console.log('=====================================================');
console.log(' Big Buck Bunny Self-Contained H.264 Streamer (Node.js)');
console.log(` Target Board IP: ${SERVER_HOST}:${SERVER_PORT}`);
console.log(` Stream Resolution: ${VID_W}x${VID_H} @ 30 FPS`);
console.log(' Source Video: client/assets/bigbuckbunny_1080p.mp4');
console.log('=====================================================\n');

const mp4Path = path.join(__dirname, 'assets', 'bigbuckbunny_1080p.mp4');
const h264Path = path.join(__dirname, 'assets', 'bigbuckbunny.h264');

if (!fs.existsSync(h264Path)) {
  if (!fs.existsSync(mp4Path)) {
    console.error(`[ERROR] MP4 video file not found at: ${mp4Path}`);
    process.exit(1);
  }
  console.log('[H.264 CONVERSION] Converting MP4 to Annex-B H.264 2Mbps stream with Access Unit Delimiters (-aud 1)...');
  execSync(`"${ffmpegPath}" -y -i "${mp4Path}" -vf scale=${VID_W}:${VID_H} -c:v libx264 -preset ultrafast -tune zerolatency -b:v 2M -maxrate 2.5M -bufsize 2M -g 30 -keyint_min 30 -sc_threshold 0 -aud 1 -bsf:v h264_mp4toannexb "${h264Path}"`, { stdio: 'inherit' });
  console.log('[H.264 CONVERSION] Annex-B H.264 stream successfully created!');
}

const h264Buf = fs.readFileSync(h264Path);

// Scan and split Annex-B NAL units by 0x00000001 start codes
const nalUnits = [];
let lastIdx = 0;

for (let i = 0; i < h264Buf.length - 4; i++) {
  if (h264Buf[i] === 0 && h264Buf[i+1] === 0 && h264Buf[i+2] === 0 && h264Buf[i+3] === 1) {
    if (i > lastIdx) {
      nalUnits.push(h264Buf.subarray(lastIdx, i));
    }
    lastIdx = i;
  }
}
if (lastIdx < h264Buf.length) {
  nalUnits.push(h264Buf.subarray(lastIdx));
}

let currentFrameNals = [];
const h264VideoFrames = [];

for (const nal of nalUnits) {
  const nalType = nal[4] & 0x1f;
  if (nalType === 9 && currentFrameNals.length > 0) { // AUD (Access Unit Delimiter) = Start of new video frame
    h264VideoFrames.push(Buffer.concat(currentFrameNals));
    currentFrameNals = [];
  }
  currentFrameNals.push(nal);
}
if (currentFrameNals.length > 0) {
  h264VideoFrames.push(Buffer.concat(currentFrameNals));
}

console.log(`[H.264 READY] Extracted ${h264VideoFrames.length} complete self-contained H.264 video frames into RAM!`);
console.log('[STEP 2] Starting 30.0 FPS Live Video Transmission...\n');

const udpClient = dgram.createSocket('udp4');
try { udpClient.setSendBufferSize(8 * 1024 * 1024); } catch (e) {}
const CHUNK_SIZE = 8192;

let frameSeq = 0;
let frameIdx = 0;

const startTime = Date.now();

const streamInterval = setInterval(() => {
  frameSeq++;
  const frameData = h264VideoFrames[frameIdx % h264VideoFrames.length];
  frameIdx++;

  sendH264Frame(frameData, frameSeq);

  if (frameSeq % 30 === 0) {
    const elapsedSec = (Date.now() - startTime) / 1000.0;
    const actualFps = (frameSeq / elapsedSec).toFixed(1);
    console.log(`[30 FPS H.264 STREAM] Frame #${frameSeq}: Transmitted ${VID_W}x${VID_H} Self-Contained H.264 Frame (${Math.round(frameData.length / 1024)} KB) -> ${SERVER_HOST}:${SERVER_PORT} (${actualFps} FPS)`);
  }
}, 33.33);

// Stop after 10 seconds of streaming
setTimeout(() => {
  clearInterval(streamInterval);
  console.log(`\n=====================================================`);
  console.log(` [STREAM COMPLETE] Streamed ${frameSeq} H.264 video frames to board.`);
  console.log(`=====================================================\n`);
  setTimeout(() => {
    udpClient.close();
    process.exit(0);
  }, 200);
}, 10000);

function sendH264Frame(frameBuf, seq) {
  const CHUNK_SIZE = 1350;
  const totalChunks = Math.ceil(frameBuf.length / CHUNK_SIZE);

  for (let c = 0; c < totalChunks; c++) {
    const start = c * CHUNK_SIZE;
    const end = Math.min(start + CHUNK_SIZE, frameBuf.length);
    const chunkData = frameBuf.subarray(start, end);

    const header = Buffer.alloc(16);
    header.write('H264', 0, 4, 'ascii');
    header.writeUInt32BE(seq, 4);
    header.writeUInt16BE(c, 8);
    header.writeUInt16BE(totalChunks, 10);
    header.writeUInt16BE(VID_W, 12);
    header.writeUInt16BE(VID_H, 14);

    const packet = Buffer.concat([header, chunkData]);
    udpClient.send(packet, SERVER_PORT, SERVER_HOST, () => {});
  }
}
