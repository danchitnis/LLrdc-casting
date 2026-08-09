interface CapturableCanvas extends HTMLCanvasElement {
  captureStream(frameRate?: number): MediaStream;
}

export interface SyntheticStreamConfig {
  width: number;
  height: number;
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
  let canvas = document.getElementById('screenCanvas') as HTMLCanvasElement | null;
  if (!canvas) {
    canvas = document.createElement('canvas');
    canvas.id = 'screenCanvas';
    canvas.style.display = 'none';
    document.body.appendChild(canvas);
  }
  canvas.width = width;
  canvas.height = height;
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
    ctx.fillRect(0, 0, width, height);

    ctx.strokeStyle = '#1e293b';
    ctx.lineWidth = 2;
    for (let x = 0; x < width; x += 100) {
      ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, height); ctx.stroke();
    }
    for (let y = 0; y < height; y += 100) {
      ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(width, y); ctx.stroke();
    }

    const time = frame / fps;
    const x = (width / 2) + Math.cos(time * 2) * (width / 3);
    const y = (height / 2) + Math.sin(time * 3) * (height / 3);

    const grad = ctx.createRadialGradient(x, y, 10, x, y, 120);
    grad.addColorStop(0, '#38bdf8');
    grad.addColorStop(1, 'rgba(56, 189, 248, 0)');
    ctx.fillStyle = grad;
    ctx.beginPath(); ctx.arc(x, y, 120, 0, Math.PI * 2); ctx.fill();

    const margin = Math.max(24, Math.round(Math.min(width, height) * 0.05));
    const titleSize = Math.max(24, Math.round(Math.min(width, height) * 0.045));
    const detailSize = Math.max(18, Math.round(titleSize * 0.62));
    const lineHeight = Math.round(detailSize * 1.5);
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
