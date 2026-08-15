import { ANNEXB_CONFIG } from './config.ts';

export interface NalCache {
  vps: Uint8Array | null;
  sps: Uint8Array | null;
  pps: Uint8Array | null;
}

export function createNalCache(): NalCache {
  return { vps: null, sps: null, pps: null };
}

export interface DecoderDescriptionSource {
  buffer?: ArrayBuffer;
  byteOffset?: number;
  byteLength?: number;
  size?: number;
}

export function parseDecoderDescription(
  description: AllowSharedBufferSource | DecoderDescriptionSource | ArrayBuffer | DataView | Uint8Array | undefined,
  codec: string,
  cache: NalCache,
  logFn?: (msg: string, isError?: boolean) => void
): void {
  if (!description) return;
  try {
    const descObj = description as DecoderDescriptionSource;
    const buffer: ArrayBuffer = descObj.buffer ? descObj.buffer : (description as ArrayBuffer);
    const byteOffset = descObj.byteOffset || 0;
    const byteLength = descObj.byteLength || descObj.size || 0;
    if (byteLength < ANNEXB_CONFIG.DECODER_DESCRIPTION_MIN_BYTES) return;
    const view = new DataView(buffer, byteOffset, byteLength);

    if (codec === 'H265') {
      if (byteLength <= ANNEXB_CONFIG.HEVC_NUM_ARRAYS_OFFSET) return;
      const numOfArrays = view.getUint8(ANNEXB_CONFIG.HEVC_NUM_ARRAYS_OFFSET);
      let offset = ANNEXB_CONFIG.HEVC_NUM_ARRAYS_OFFSET + 1;

      for (let i = 0; i < numOfArrays; i++) {
        if (offset + ANNEXB_CONFIG.HEVC_ARRAY_HEADER_BYTES > byteLength) break;
        const nalType = view.getUint8(offset) & 0x3f;
        const numNalus = view.getUint16(offset + 1, false);
        offset += ANNEXB_CONFIG.HEVC_ARRAY_HEADER_BYTES;

        for (let j = 0; j < numNalus; j++) {
          if (offset + ANNEXB_CONFIG.NAL_LENGTH_BYTES > byteLength) break;
          const nalLen = view.getUint16(offset, false);
          offset += ANNEXB_CONFIG.NAL_LENGTH_BYTES;
          if (offset + nalLen > byteLength) break;

          const nalData = new Uint8Array(buffer, byteOffset + offset, nalLen);
          if (nalType === ANNEXB_CONFIG.H265_VPS_TYPE) cache.vps = nalData;
          if (nalType === ANNEXB_CONFIG.H265_SPS_TYPE) cache.sps = nalData;
          if (nalType === ANNEXB_CONFIG.H265_PPS_TYPE) cache.pps = nalData;
          offset += nalLen;
        }
      }
      logFn?.(`[HEADER PARSED] HEVC VPS=${!!cache.vps} (${cache.vps?.length}B) SPS=${!!cache.sps} (${cache.sps?.length}B) PPS=${!!cache.pps} (${cache.pps?.length}B)`);
    } else {
      const numOfSps = view.getUint8(ANNEXB_CONFIG.AVC_NUM_SPS_OFFSET) & 0x1f;
      let offset = ANNEXB_CONFIG.AVC_NUM_SPS_OFFSET + 1;
      for (let i = 0; i < numOfSps; i++) {
        if (offset + ANNEXB_CONFIG.NAL_LENGTH_BYTES > byteLength) break;
        const spsLen = view.getUint16(offset, false);
        offset += ANNEXB_CONFIG.NAL_LENGTH_BYTES;
        if (offset + spsLen > byteLength) break;
        cache.sps = new Uint8Array(buffer, byteOffset + offset, spsLen);
        offset += spsLen;
      }
      if (offset + 1 > byteLength) return;
      const numOfPps = view.getUint8(offset);
      offset += 1;
      for (let i = 0; i < numOfPps; i++) {
        if (offset + ANNEXB_CONFIG.NAL_LENGTH_BYTES > byteLength) break;
        const ppsLen = view.getUint16(offset, false);
        offset += ANNEXB_CONFIG.NAL_LENGTH_BYTES;
        if (offset + ppsLen > byteLength) break;
        cache.pps = new Uint8Array(buffer, byteOffset + offset, ppsLen);
        offset += ppsLen;
      }
      logFn?.(`[HEADER PARSED] H264 SPS=${!!cache.sps} (${cache.sps?.length}B) PPS=${!!cache.pps} (${cache.pps?.length}B)`);
    }
  } catch (err) {
    const error = err as Error;
    logFn?.(`[HEADER PARSER ERROR] ${error.message}`, true);
  }
}

export function convertToAnnexB(
  chunk: EncodedVideoChunk,
  metadata: EncodedVideoChunkMetadata | undefined,
  codec: string,
  cache: NalCache,
  seqNum: number,
  logFn?: (msg: string, isError?: boolean) => void
): Uint8Array {
  const chunkBuffer = new Uint8Array(chunk.byteLength);
  chunk.copyTo(chunkBuffer);

  if (metadata && metadata.decoderConfig && metadata.decoderConfig.description) {
    parseDecoderDescription(metadata.decoderConfig.description, codec, cache, logFn);
  }

  const rawNalTypes: number[] = [];
  const view = new DataView(chunkBuffer.buffer, chunkBuffer.byteOffset, chunkBuffer.byteLength);
  let scanOffset = 0;
  while (scanOffset + 4 <= chunkBuffer.length) {
    const nalLen = view.getUint32(scanOffset, false);
    scanOffset += ANNEXB_CONFIG.NAL_LENGTH_PREFIX_BYTES;
    if (nalLen === 0 || scanOffset + nalLen > chunkBuffer.length) break;
    const headerByte = chunkBuffer[scanOffset];
    const type = codec === 'H265' ? ((headerByte >> 1) & 0x3f) : (headerByte & 0x1f);
    rawNalTypes.push(type);
    scanOffset += nalLen;
  }

  const isKey = (chunk.type === 'key');
  if (isKey || seqNum === 1) {
    const toHex = (arr: Uint8Array | null) => arr ? Array.from(arr).map(b => b.toString(16).padStart(2, '0')).join('') : 'NULL';
    logFn?.(`[KEYFRAME PREPEND HEX] seq=${seqNum} VPS=${toHex(cache.vps)} SPS=${toHex(cache.sps)} PPS=${toHex(cache.pps)}`);
  }
  const startCode = new Uint8Array(ANNEXB_CONFIG.START_CODE);
  const nalParts: Uint8Array[] = [];

  if (codec === 'H265') {
    nalParts.push(new Uint8Array([...ANNEXB_CONFIG.START_CODE, 0x46, 0x01, 0x50]));
  } else {
    nalParts.push(new Uint8Array([...ANNEXB_CONFIG.START_CODE, ANNEXB_CONFIG.H264_AUD, 0xf0]));
  }

  if (isKey || seqNum === 1) {
    if (codec === 'H265') {
      if (cache.vps) { nalParts.push(startCode); nalParts.push(cache.vps); }
      if (cache.sps) { nalParts.push(startCode); nalParts.push(cache.sps); }
      if (cache.pps) { nalParts.push(startCode); nalParts.push(cache.pps); }
    } else {
      if (cache.sps) { nalParts.push(startCode); nalParts.push(cache.sps); }
      if (cache.pps) { nalParts.push(startCode); nalParts.push(cache.pps); }
    }
  }

  let offset = 0;
  while (offset + 4 <= chunkBuffer.length) {
    const nalLen = view.getUint32(offset, false);
    offset += ANNEXB_CONFIG.NAL_LENGTH_PREFIX_BYTES;
    if (nalLen === 0 || offset + nalLen > chunkBuffer.length) {
      if (chunkBuffer[0] === 0 && chunkBuffer[1] === 0 && (chunkBuffer[2] === 1 || (chunkBuffer[2] === 0 && chunkBuffer[3] === 1))) {
        nalParts.push(chunkBuffer);
      } else {
        nalParts.push(startCode);
        nalParts.push(chunkBuffer.subarray(offset - 4));
      }
      break;
    }
    nalParts.push(startCode);
    nalParts.push(chunkBuffer.subarray(offset, offset + nalLen));
    offset += nalLen;
  }

  let totalLen = 0;
  for (const part of nalParts) totalLen += part.length;

  const result = new Uint8Array(totalLen);
  let pos = 0;
  for (const part of nalParts) {
    result.set(part, pos);
    pos += part.length;
  }
  return result;
}
