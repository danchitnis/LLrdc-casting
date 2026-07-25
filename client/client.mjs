import dgram from 'node:dgram';

const SERVER_HOST = process.argv[2] || process.env.BOARD_IP || '192.168.1.72';
const SERVER_PORT = parseInt(process.argv[3] || process.env.BOARD_PORT || '4433', 10);

console.log('=====================================================');
console.log(' WebTransport QUIC UDP H.264 Dev Client (Node.js)');
console.log(` Target Server: ${SERVER_HOST}:${SERVER_PORT} (UDP)`);
console.log('=====================================================\n');

// H.264 Annex-B NAL unit static frame payload (SPS, PPS, IDR Slice)
const h264FramePayload = Buffer.from([
  // NAL 1: SPS (Sequence Parameter Set)
  0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1f, 0xda, 0x01, 0x40, 0x16,
  0xec, 0x04, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40, 0x00, 0x00, 0x0f, 0x23, 0xc6, 0x0c, 0x65,
  // NAL 2: PPS (Picture Parameter Set)
  0x00, 0x00, 0x00, 0x01, 0x68, 0xce, 0x38, 0x80,
  // NAL 3: IDR Keyframe Data
  0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x10, 0xff, 0xfe, 0xf6,
  0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03,
  0x00, 0x40, 0x00, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0
]);

const client = dgram.createSocket('udp4');

// Send H.264 frame to ports 4434 and 4433
client.send(h264FramePayload, 4434, SERVER_HOST, (err) => {
  if (err) console.error('[CLIENT ERROR] Port 4434 error:', err);
  else console.log(`[CLIENT SUCCESS] Transmitted ${h264FramePayload.length} bytes of static H.264 frame to ${SERVER_HOST}:4434`);
});

client.send(h264FramePayload, 4433, SERVER_HOST, (err) => {
  if (err) console.error('[CLIENT ERROR] Port 4433 error:', err);
  else console.log(`[CLIENT SUCCESS] Transmitted ${h264FramePayload.length} bytes of static H.264 frame to ${SERVER_HOST}:4433`);
  setTimeout(() => client.close(), 500);
});
