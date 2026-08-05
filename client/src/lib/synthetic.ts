interface CapturableCanvas extends HTMLCanvasElement {
  captureStream(frameRate?: number): MediaStream;
}

export function createSyntheticScreenStream(
  width: number,
  height: number,
  fps: number,
  isStreamingCheck: () => boolean
): MediaStream {
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

    ctx.fillStyle = '#ffffff';
    ctx.font = 'bold 48px monospace';
    ctx.fillText(`LLrdc Casting HARDWARE HEVC STREAM`, 100, 120);
    ctx.font = '36px monospace';
    ctx.fillText(`Resolution: ${width}x${height} @ ${fps} FPS`, 100, 180);
    ctx.fillText(`Frame #${frame} | Time: ${time.toFixed(2)}s`, 100, 240);
  }, intervalMs);

  return (canvas as CapturableCanvas).captureStream(fps);
}
