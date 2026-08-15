import { SYNTHETIC_PATTERN_CONFIG } from './config.ts';

interface CapturableCanvas extends HTMLCanvasElement {
  captureStream(frameRate?: number): MediaStream;
}

export interface SyntheticStreamConfig {
  width: number;
  height: number;
  /** Canvas dimensions used by the encoder.  Synthetic output may need the
   * codec-aligned height (for example 1920x1088 for the 1080p choice) while
   * the user-facing source remains 1920x1080. */
  renderWidth?: number;
  renderHeight?: number;
  encodedWidth: number;
  encodedHeight: number;
  fps: number;
  codec: 'H264' | 'H265';
  hardwarePreference: 'prefer-hardware' | 'prefer-software';
  bitrate: number;
  aspectMode: 'preserve' | 'stretch';
  latencyMode: 'ULL' | 'balanced' | 'quality';
}

export function formatSyntheticStatus(config: SyntheticStreamConfig, frame: number): string[] {
  const codecName = config.codec === 'H265' ? 'HEVC / H.265' : 'H.264';
  const acceleration = config.hardwarePreference === 'prefer-software' ? 'SW / CPU' : 'HW preferred';
  return [
    `${codecName} (${acceleration})`,
    `Source: ${config.width}x${config.height} | Coded: ${config.encodedWidth}x${config.encodedHeight}`,
    `Output: ${config.fps} FPS | Bitrate: ${(config.bitrate / 1_000_000).toFixed(1)} Mbps | Aspect: ${config.aspectMode}`,
    `Priority: ${config.latencyMode}`,
    `Frame #${frame} | Time: ${(frame / config.fps).toFixed(2)}s`,
  ];
}

export function createSyntheticScreenStream(
  config: SyntheticStreamConfig,
  isStreamingCheck: () => boolean
): MediaStream {
  const { width, height, fps } = config;
  const renderWidth = config.renderWidth ?? width;
  const renderHeight = config.renderHeight ?? height;
  let canvas = document.getElementById('screenCanvas') as HTMLCanvasElement | null;
  if (!canvas) {
    canvas = document.createElement('canvas');
    canvas.id = 'screenCanvas';
    canvas.style.display = 'none';
    document.body.appendChild(canvas);
  }
  canvas.width = renderWidth;
  canvas.height = renderHeight;
  const ctx = canvas.getContext('2d');

  let frame = 0;
  const intervalMs = 1000 / fps;
  const animInterval = setInterval(() => {
    if (!isStreamingCheck() || !ctx) {
      clearInterval(animInterval);
      return;
    }
    frame++;
    ctx.fillStyle = '#0f172a';
    ctx.fillRect(0, 0, renderWidth, renderHeight);

    ctx.strokeStyle = '#1e293b';
    ctx.lineWidth = SYNTHETIC_PATTERN_CONFIG.GRID_LINE_WIDTH;
    for (let x = 0; x < renderWidth; x += SYNTHETIC_PATTERN_CONFIG.GRID_STEP) {
      ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, renderHeight); ctx.stroke();
    }
    for (let y = 0; y < renderHeight; y += SYNTHETIC_PATTERN_CONFIG.GRID_STEP) {
      ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(renderWidth, y); ctx.stroke();
    }

    const time = frame / fps;
    const x = (renderWidth / 2) + Math.cos(time * SYNTHETIC_PATTERN_CONFIG.ORB_X_SPEED) * (renderWidth / 3);
    const y = (renderHeight / 2) + Math.sin(time * SYNTHETIC_PATTERN_CONFIG.ORB_Y_SPEED) * (renderHeight / 3);

    const grad = ctx.createRadialGradient(x, y, SYNTHETIC_PATTERN_CONFIG.GRADIENT_RADIUS, x, y, SYNTHETIC_PATTERN_CONFIG.ORB_RADIUS);
    grad.addColorStop(0, '#38bdf8');
    grad.addColorStop(1, 'rgba(56, 189, 248, 0)');
    ctx.fillStyle = grad;
    ctx.beginPath(); ctx.arc(x, y, SYNTHETIC_PATTERN_CONFIG.ORB_RADIUS, 0, Math.PI * 2); ctx.fill();

    const margin = Math.max(SYNTHETIC_PATTERN_CONFIG.LABEL_MARGIN_MAX_PX, Math.round(Math.min(renderWidth, renderHeight) * SYNTHETIC_PATTERN_CONFIG.LABEL_MARGIN_RATIO));
    const titleSize = Math.max(SYNTHETIC_PATTERN_CONFIG.TITLE_MAX_PX, Math.round(Math.min(renderWidth, renderHeight) * SYNTHETIC_PATTERN_CONFIG.TITLE_RATIO));
    const detailSize = Math.max(SYNTHETIC_PATTERN_CONFIG.DETAIL_MIN_PX, Math.round(titleSize * SYNTHETIC_PATTERN_CONFIG.DETAIL_RATIO));
    const lineHeight = Math.round(detailSize * SYNTHETIC_PATTERN_CONFIG.LINE_HEIGHT);
    const statusLines = formatSyntheticStatus(config, frame);
    ctx.fillStyle = '#ffffff';
    ctx.font = `bold ${titleSize}px monospace`;
    ctx.fillText('LLrdc CASTING TEST PATTERN', margin, margin + titleSize);
    ctx.font = `${detailSize}px monospace`;
    statusLines.forEach((line, index) => {
      ctx.fillText(line, margin, margin + titleSize + lineHeight * (index + 1));
    });
  }, intervalMs);

  return (canvas as CapturableCanvas).captureStream(fps);
}
