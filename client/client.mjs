import dgram from 'node:dgram';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execSync } from 'node:child_process';
import ffmpegPath from 'ffmpeg-static';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// Argument Parsing:
// Options can be passed in order: HOST PORT RES FPS CODEC STREAM_FILE
// OR env vars: BOARD_IP, BOARD_PORT, BOARD_WIDTH, BOARD_HEIGHT, STREAM_FPS, CODEC, STREAM_FILE
const SERVER_HOST = process.argv[2] || process.env.BOARD_IP || '192.168.1.72';
const SERVER_PORT = parseInt(process.argv[3] || process.env.BOARD_PORT || '4434', 10);

let VID_W = parseInt(process.env.BOARD_WIDTH || '1280', 10);
let VID_H = parseInt(process.env.BOARD_HEIGHT || '720', 10);
let FPS = parseInt(process.env.STREAM_FPS || '30', 10);
let CODEC = (process.env.CODEC || 'H264').toUpperCase();
let streamFilePath = process.env.STREAM_FILE || null;

let argIdx = 4;
if (process.argv[argIdx]) {
  const arg = process.argv[argIdx];
  if (arg.includes('x')) {
    const parts = arg.split('x');
    VID_W = parseInt(parts[0], 10);
    VID_H = parseInt(parts[1], 10);
    argIdx++;
  } else if (!isNaN(parseInt(arg, 10)) && process.argv[argIdx + 1] && !isNaN(parseInt(process.argv[argIdx + 1], 10))) {
    VID_W = parseInt(arg, 10);
    VID_H = parseInt(process.argv[argIdx + 1], 10);
    argIdx += 2;
  }
}

if (process.argv[argIdx] && !isNaN(parseInt(process.argv[argIdx], 10))) {
  FPS = parseInt(process.argv[argIdx], 10);
  argIdx++;
}

if (process.argv[argIdx] && (process.argv[argIdx].toUpperCase() === 'H264' || process.argv[argIdx].toUpperCase() === 'H265' || process.argv[argIdx].toUpperCase() === 'HEVC')) {
  CODEC = process.argv[argIdx].toUpperCase();
  if (CODEC === 'HEVC') CODEC = 'H265';
  argIdx++;
}

if (process.argv[argIdx]) {
  streamFilePath = process.argv[argIdx];
}

// Default stream file if not specified
if (!streamFilePath) {
  const codecExt = CODEC === 'H265' ? '265' : '264';
  const defaultPrepared = path.join(__dirname, 'assets', `stream_${VID_W}x${VID_H}_${FPS}fps_${CODEC.toLowerCase()}.${codecExt}`);
  const fallbackH264 = path.join(__dirname, 'assets', 'bigbuckbunny.h264');
  
  if (fs.existsSync(defaultPrepared)) {
    streamFilePath = defaultPrepared;
  } else if (fs.existsSync(fallbackH264)) {
    streamFilePath = fallbackH264;
  } else {
    streamFilePath = defaultPrepared;
  }
}

// Auto-prepare file if missing
if (!fs.existsSync(streamFilePath)) {
  const mp4Path = path.join(__dirname, 'assets', `bigbuckbunny_${VID_W}x${VID_H}.mp4`);
  const masterMp4 = path.join(__dirname, 'assets', 'bigbuckbunny_1080p.mp4');
  const sourceMp4 = fs.existsSync(mp4Path) ? mp4Path : masterMp4;

  if (!fs.existsSync(sourceMp4)) {
    console.error(`[ERROR] Source video file not found at ${sourceMp4}`);
    process.exit(1);
  }

  console.log(`[VIDEO CONVERSION] Converting MP4 to Annex-B ${CODEC} stream (${VID_W}x${VID_H} @ ${FPS} FPS)...`);
  const bsf = CODEC === 'H265' ? 'hevc_mp4toannexb' : 'h264_mp4toannexb';
  const cLib = CODEC === 'H265' ? 'libx265' : 'libx264';
  const extraParams = CODEC === 'H265' ? `-x265-params "keyint=${FPS}:min-keyint=${FPS}:no-scenecut=1:aud=1:repeat-headers=1"` : `-x264-params "keyint=${FPS}:min-keyint=${FPS}:no-scenecut=1:repeat-headers=1" -g ${FPS} -keyint_min ${FPS} -sc_threshold 0`;

  execSync(`"${ffmpegPath}" -y -i "${sourceMp4}" -vf scale=${VID_W}:${VID_H} -r ${FPS} -c:v ${cLib} -preset ultrafast -tune zerolatency -b:v 2M -maxrate 2.5M -bufsize 2M ${extraParams} -aud 1 -bsf:v ${bsf} "${streamFilePath}"`, { stdio: 'inherit' });
  console.log(`[VIDEO CONVERSION] Annex-B ${CODEC} stream successfully created!`);
}

console.log('=====================================================');
console.log(` Big Buck Bunny Video Streamer Client (${CODEC})`);
console.log(` Target Board IP  : ${SERVER_HOST}:${SERVER_PORT}`);
console.log(` Stream Resolution: ${VID_W}x${VID_H} @ ${FPS} FPS`);
console.log(` Codec            : ${CODEC}`);
console.log(` Stream Bitstream : ${streamFilePath}`);
console.log('=====================================================\n');

const streamBuf = fs.readFileSync(streamFilePath);

function getNalType(nal, codec) {
  let headerOffset = 3;
  if (nal.length >= 4 && nal[0] === 0 && nal[1] === 0 && nal[2] === 0 && nal[3] === 1) {
    headerOffset = 4;
  }
  if (nal.length <= headerOffset) return -1;
  const headerByte = nal[headerOffset];
  if (codec === 'H265') {
    return (headerByte >> 1) & 0x3f;
  } else {
    return headerByte & 0x1f;
  }
}

// Scan and split Annex-B NAL units by 3-byte (0x000001) or 4-byte (0x00000001) start codes
const nalUnits = [];
let scanIdx = 0;
let lastStart = -1;

while (scanIdx <= streamBuf.length - 3) {
  let isStart = false;
  let startLen = 0;

  if (streamBuf[scanIdx] === 0 && streamBuf[scanIdx + 1] === 0) {
    if (streamBuf[scanIdx + 2] === 1) {
      isStart = true;
      startLen = 3;
    } else if (scanIdx <= streamBuf.length - 4 && streamBuf[scanIdx + 2] === 0 && streamBuf[scanIdx + 3] === 1) {
      isStart = true;
      startLen = 4;
    }
  }

  if (isStart) {
    if (lastStart !== -1) {
      nalUnits.push(streamBuf.subarray(lastStart, scanIdx));
    }
    lastStart = scanIdx;
    scanIdx += startLen;
  } else {
    scanIdx++;
  }
}
if (lastStart !== -1 && lastStart < streamBuf.length) {
  nalUnits.push(streamBuf.subarray(lastStart));
}

let vpsNal = null;
let spsNal = null;
let ppsNal = null;
let h264SpsNal = null;
let h264PpsNal = null;

let currentFrameNals = [];
const videoFrames = [];

for (const nal of nalUnits) {
  const nalType = getNalType(nal, CODEC);
  if (nalType === -1) continue;

  let isAud = false;

  if (CODEC === 'H265') {
    if (nalType === 35) isAud = true; // HEVC AUD_NUT = 35
    if (nalType === 32) vpsNal = nal; // VPS
    if (nalType === 33) spsNal = nal; // SPS
    if (nalType === 34) ppsNal = nal; // PPS
  } else {
    if (nalType === 9) isAud = true; // H.264 AUD = 9
    if (nalType === 7) h264SpsNal = nal;
    if (nalType === 8) h264PpsNal = nal;
  }

  if (isAud && currentFrameNals.length > 0) {
    if (CODEC === 'H265') {
      const hasHeader = currentFrameNals.some(n => {
        const t = getNalType(n, 'H265');
        return t === 32 || t === 33 || t === 34;
      });
      const headerList = [vpsNal, spsNal, ppsNal].filter(Boolean);
      if (!hasHeader && headerList.length > 0) {
        videoFrames.push(Buffer.concat([...headerList, ...currentFrameNals]));
      } else {
        videoFrames.push(Buffer.concat(currentFrameNals));
      }
    } else {
      const hasHeader = currentFrameNals.some(n => {
        const t = getNalType(n, 'H264');
        return t === 7 || t === 8;
      });
      const headerList = [h264SpsNal, h264PpsNal].filter(Boolean);
      if (!hasHeader && headerList.length > 0) {
        videoFrames.push(Buffer.concat([...headerList, ...currentFrameNals]));
      } else {
        videoFrames.push(Buffer.concat(currentFrameNals));
      }
    }
    currentFrameNals = [];
  }
  currentFrameNals.push(nal);
}

if (currentFrameNals.length > 0) {
  if (CODEC === 'H265') {
    const hasHeader = currentFrameNals.some(n => {
      const t = getNalType(n, 'H265');
      return t === 32 || t === 33 || t === 34;
    });
    const headerList = [vpsNal, spsNal, ppsNal].filter(Boolean);
    if (!hasHeader && headerList.length > 0) {
      videoFrames.push(Buffer.concat([...headerList, ...currentFrameNals]));
    } else {
      videoFrames.push(Buffer.concat(currentFrameNals));
    }
  } else {
    const hasHeader = currentFrameNals.some(n => {
      const t = getNalType(n, 'H264');
      return t === 7 || t === 8;
    });
    const headerList = [h264SpsNal, h264PpsNal].filter(Boolean);
    if (!hasHeader && headerList.length > 0) {
      videoFrames.push(Buffer.concat([...headerList, ...currentFrameNals]));
    } else {
      videoFrames.push(Buffer.concat(currentFrameNals));
    }
  }
}

console.log(`[STREAM READY] Extracted ${videoFrames.length} complete self-contained ${CODEC} video frames into RAM!`);
console.log(`[STEP 2] Starting ${FPS}.0 FPS Live Video Transmission...\n`);

const udpClient = dgram.createSocket('udp4');
try { udpClient.setSendBufferSize(8 * 1024 * 1024); } catch (e) {}

let frameSeq = 0;
let frameIdx = 0;

const startTime = Date.now();
const intervalMs = 1000.0 / FPS;

const streamInterval = setInterval(() => {
  frameSeq++;
  const frameData = videoFrames[frameIdx % videoFrames.length];
  frameIdx++;

  sendVideoFrame(frameData, frameSeq);

  if (frameSeq % FPS === 0) {
    const elapsedSec = (Date.now() - startTime) / 1000.0;
    const actualFps = (frameSeq / elapsedSec).toFixed(1);
    console.log(`[${FPS} FPS ${CODEC} STREAM] Frame #${frameSeq}: Transmitted ${VID_W}x${VID_H} Frame (${Math.round(frameData.length / 1024)} KB) -> ${SERVER_HOST}:${SERVER_PORT} (${actualFps} FPS)`);
  }
}, intervalMs);

// Stop after 10 seconds of streaming
setTimeout(() => {
  clearInterval(streamInterval);
  console.log(`\n=====================================================`);
  console.log(` [STREAM COMPLETE] Streamed ${frameSeq} ${CODEC} video frames to board.`);
  console.log(`=====================================================\n`);
  setTimeout(() => {
    udpClient.close();
    process.exit(0);
  }, 200);
}, 10000);

function sendVideoFrame(frameBuf, seq) {
  const CHUNK_SIZE = 1350;
  const totalChunks = Math.ceil(frameBuf.length / CHUNK_SIZE);
  const headerTag = CODEC === 'H265' ? 'H265' : 'H264';

  for (let c = 0; c < totalChunks; c++) {
    const start = c * CHUNK_SIZE;
    const end = Math.min(start + CHUNK_SIZE, frameBuf.length);
    const chunkData = frameBuf.subarray(start, end);

    const header = Buffer.alloc(16);
    header.write(headerTag, 0, 4, 'ascii');
    header.writeUInt32BE(seq, 4);
    header.writeUInt16BE(c, 8);
    header.writeUInt16BE(totalChunks, 10);
    header.writeUInt16BE(VID_W, 12);
    header.writeUInt16BE(VID_H, 14);

    const packet = Buffer.concat([header, chunkData]);
    udpClient.send(packet, SERVER_PORT, SERVER_HOST, () => {});
  }
}
