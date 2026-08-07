export type AspectMode = 'preserve' | 'stretch';

export interface CompositorLayout {
  sourceWidth: number;
  sourceHeight: number;
  targetWidth: number;
  targetHeight: number;
  contentX: number;
  contentY: number;
  contentWidth: number;
  contentHeight: number;
}

function positiveDimension(value: number): number {
  return Number.isFinite(value) && value > 0 ? Math.floor(value) : 1;
}

export function calculateCompositorLayout(
  sourceWidth: number,
  sourceHeight: number,
  targetWidth: number,
  targetHeight: number,
  aspectMode: AspectMode,
): CompositorLayout {
  const safeSourceWidth = positiveDimension(sourceWidth);
  const safeSourceHeight = positiveDimension(sourceHeight);
  const safeTargetWidth = positiveDimension(targetWidth);
  const safeTargetHeight = positiveDimension(targetHeight);

  if (aspectMode === 'stretch') {
    return {
      sourceWidth: safeSourceWidth,
      sourceHeight: safeSourceHeight,
      targetWidth: safeTargetWidth,
      targetHeight: safeTargetHeight,
      contentX: 0,
      contentY: 0,
      contentWidth: safeTargetWidth,
      contentHeight: safeTargetHeight,
    };
  }

  const scale = Math.min(safeTargetWidth / safeSourceWidth, safeTargetHeight / safeSourceHeight);
  const contentWidth = Math.max(1, Math.round(safeSourceWidth * scale));
  const contentHeight = Math.max(1, Math.round(safeSourceHeight * scale));

  return {
    sourceWidth: safeSourceWidth,
    sourceHeight: safeSourceHeight,
    targetWidth: safeTargetWidth,
    targetHeight: safeTargetHeight,
    contentX: Math.floor((safeTargetWidth - contentWidth) / 2),
    contentY: Math.floor((safeTargetHeight - contentHeight) / 2),
    contentWidth,
    contentHeight,
  };
}

export function formatContentRect(layout: CompositorLayout): string {
  return `<${layout.contentX},${layout.contentY},${layout.contentWidth},${layout.contentHeight}>`;
}

export class VideoFrameCompositor {
  private readonly canvas: OffscreenCanvas;
  private readonly context: OffscreenCanvasRenderingContext2D;
  private readonly targetWidth: number;
  private readonly targetHeight: number;
  private readonly aspectMode: AspectMode;

  constructor(targetWidth: number, targetHeight: number, aspectMode: AspectMode) {
    this.targetWidth = targetWidth;
    this.targetHeight = targetHeight;
    this.aspectMode = aspectMode;
    this.canvas = new OffscreenCanvas(targetWidth, targetHeight);

    const context = this.canvas.getContext('2d', { alpha: false });
    if (!context) {
      throw new Error('OffscreenCanvas 2D compositor is unavailable');
    }
    this.context = context;
    this.context.imageSmoothingEnabled = true;
    this.context.imageSmoothingQuality = 'high';
  }

  layoutFor(sourceWidth: number, sourceHeight: number): CompositorLayout {
    return calculateCompositorLayout(
      sourceWidth,
      sourceHeight,
      this.targetWidth,
      this.targetHeight,
      this.aspectMode,
    );
  }

  compose(frame: VideoFrame): VideoFrame {
    const layout = this.layoutFor(frame.displayWidth, frame.displayHeight);

    if (this.aspectMode === 'preserve') {
      this.context.fillStyle = '#000000';
      this.context.fillRect(0, 0, this.targetWidth, this.targetHeight);
    }

    this.context.drawImage(
      frame,
      0,
      0,
      layout.sourceWidth,
      layout.sourceHeight,
      layout.contentX,
      layout.contentY,
      layout.contentWidth,
      layout.contentHeight,
    );

    return new VideoFrame(this.canvas, {
      timestamp: frame.timestamp,
      alpha: 'discard',
    });
  }
}
