import dgram from 'node:dgram';

const SERVER_HOST = process.argv[2] || process.env.BOARD_IP || '192.168.1.72';
const SERVER_PORT = parseInt(process.argv[3] || process.env.BOARD_PORT || '4434', 10);

console.log('=====================================================');
console.log(' WebTransport / UDP Live Video Streamer (Node.js)');
console.log(` Target Board IP: ${SERVER_HOST}:${SERVER_PORT}`);
console.log(' Streaming 30 FPS Live Video for 5 SECONDS...');
console.log('=====================================================\n');

const udpClient = dgram.createSocket('udp4');

const VID_W = 320;
const VID_H = 180;
const CHUNK_SIZE = 8000;

let frameSeq = 0;

// Generate and stream an animated video frame every 33ms (30 FPS)
async function sendVideoFrame() {
  frameSeq++;
  const time = Date.now() / 1000.0;

  const rgbBuffer = Buffer.alloc(VID_W * VID_H * 3);

  // Generate animated video scene (moving ocean waves & glowing golden sphere)
  const ballX = Math.floor(VID_W / 2 + Math.sin(time * 3) * (VID_W / 3));
  const ballY = Math.floor(VID_H / 2 + Math.cos(time * 2) * (VID_H / 3));
  const ballR = 25;

  for (let y = 0; y < VID_H; y++) {
    for (let x = 0; x < VID_W; x++) {
      const idx = (y * VID_W + x) * 3;

      const dx = x - ballX;
      const dy = y - ballY;
      const distSq = dx * dx + dy * dy;

      if (distSq <= ballR * ballR) {
        // Glowing Golden Ball
        rgbBuffer[idx] = 255;
        rgbBuffer[idx + 1] = 200;
        rgbBuffer[idx + 2] = 0;
      } else {
        // Ocean Wave Colors
        const wave = Math.sin(x * 0.05 + time * 4) * 20;
        const r = Math.floor(10 + Math.sin(y * 0.02 + time) * 10);
        const g = Math.floor(100 + wave + (y / VID_H) * 80);
        const b = Math.floor(200 + (x / VID_W) * 55);

        rgbBuffer[idx] = Math.min(255, Math.max(0, r));
        rgbBuffer[idx + 1] = Math.min(255, Math.max(0, g));
        rgbBuffer[idx + 2] = Math.min(255, Math.max(0, b));
      }
    }
  }

  // Slice RGB frame into VIDC chunks and send over UDP with micro-pacing
  const totalChunks = Math.ceil(rgbBuffer.length / CHUNK_SIZE);

  for (let c = 0; c < totalChunks; c++) {
    const start = c * CHUNK_SIZE;
    const end = Math.min(start + CHUNK_SIZE, rgbBuffer.length);
    const chunkData = rgbBuffer.subarray(start, end);

    // Header: "VIDC" (4B), FrameSeq (4B), ChunkIdx (2B), TotalChunks (2B), Width (2B), Height (2B)
    const header = Buffer.alloc(16);
    header.write('VIDC', 0, 4, 'ascii');
    header.writeUInt32BE(frameSeq, 4);
    header.writeUInt16BE(c, 8);
    header.writeUInt16BE(totalChunks, 10);
    header.writeUInt16BE(VID_W, 12);
    header.writeUInt16BE(VID_H, 14);

    const packet = Buffer.concat([header, chunkData]);

    udpClient.send(packet, SERVER_PORT, SERVER_HOST, () => {});

    if (c < totalChunks - 1) {
      await new Promise((r) => setTimeout(r, 1));
    }
  }

  console.log(`[LIVE VIDEO STREAMING] Frame #${frameSeq}: Transmitted 320x180 RGB frame -> ${SERVER_HOST}:${SERVER_PORT}`);
}

// Stream live video at 30 FPS
const streamInterval = setInterval(() => {
  sendVideoFrame();
}, 33);

// Automatically stop streaming after 5 seconds
setTimeout(() => {
  clearInterval(streamInterval);
  console.log(`\n=====================================================`);
  console.log(` [CLIENT COMPLETE] Streamed ${frameSeq} video frames (5 seconds) to board.`);
  console.log(`=====================================================\n`);
  setTimeout(() => {
    udpClient.close();
    process.exit(0);
  }, 200);
}, 5000);
