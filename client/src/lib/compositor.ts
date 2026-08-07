export type AspectMode = 'preserve' | 'stretch';

export interface DisplayGeometry {
  signalWidth: number;
  signalHeight: number;
  panelWidth: number;
  panelHeight: number;
}

export interface CompositorLayout {
  sourceWidth: number;
  sourceHeight: number;
  targetWidth: number;
  targetHeight: number;
  contentX: number;
  contentY: number;
  contentWidth: number;
  contentHeight: number;
  signalContentX: number;
  signalContentY: number;
  signalContentWidth: number;
  signalContentHeight: number;
  panelContentX: number;
  panelContentY: number;
  panelContentWidth: number;
  panelContentHeight: number;
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
  display: DisplayGeometry,
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
      signalContentX: 0,
      signalContentY: 0,
      signalContentWidth: display.signalWidth,
      signalContentHeight: display.signalHeight,
      panelContentX: 0,
      panelContentY: 0,
      panelContentWidth: display.panelWidth,
      panelContentHeight: display.panelHeight,
    };
  }

  const panelAspect = display.panelWidth / display.panelHeight;
  const sourceAspect = safeSourceWidth / safeSourceHeight;
  const panelContentWidth = sourceAspect < panelAspect
    ? Math.max(1, Math.round(display.panelHeight * sourceAspect))
    : display.panelWidth;
  const panelContentHeight = sourceAspect < panelAspect
    ? display.panelHeight
    : Math.max(1, Math.round(display.panelWidth / sourceAspect));
  const panelContentX = Math.floor((display.panelWidth - panelContentWidth) / 2);
  const panelContentY = Math.floor((display.panelHeight - panelContentHeight) / 2);

  const signalContentX = Math.round(panelContentX * display.signalWidth / display.panelWidth);
  const signalContentY = Math.round(panelContentY * display.signalHeight / display.panelHeight);
  const signalContentWidth = Math.max(1, Math.round(panelContentWidth * display.signalWidth / display.panelWidth));
  const signalContentHeight = Math.max(1, Math.round(panelContentHeight * display.signalHeight / display.panelHeight));
  const contentX = Math.round(signalContentX * safeTargetWidth / display.signalWidth);
  const contentY = Math.round(signalContentY * safeTargetHeight / display.signalHeight);
  const contentWidth = Math.max(1, Math.round(signalContentWidth * safeTargetWidth / display.signalWidth));
  const contentHeight = Math.max(1, Math.round(signalContentHeight * safeTargetHeight / display.signalHeight));

  return {
    sourceWidth: safeSourceWidth,
    sourceHeight: safeSourceHeight,
    targetWidth: safeTargetWidth,
    targetHeight: safeTargetHeight,
    contentX,
    contentY,
    contentWidth,
    contentHeight,
    signalContentX,
    signalContentY,
    signalContentWidth,
    signalContentHeight,
    panelContentX,
    panelContentY,
    panelContentWidth,
    panelContentHeight,
  };
}

export function formatContentRect(layout: CompositorLayout): string {
  return `<${layout.contentX},${layout.contentY},${layout.contentWidth},${layout.contentHeight}>`;
}

export function formatSignalContentRect(layout: CompositorLayout): string {
  return `<${layout.signalContentX},${layout.signalContentY},${layout.signalContentWidth},${layout.signalContentHeight}>`;
}

export function formatPanelContentRect(layout: CompositorLayout): string {
  return `<${layout.panelContentX},${layout.panelContentY},${layout.panelContentWidth},${layout.panelContentHeight}>`;
}

export class VideoFrameCompositor {
  private readonly canvas: OffscreenCanvas;
  private readonly context: OffscreenCanvasRenderingContext2D;
  private readonly targetWidth: number;
  private readonly targetHeight: number;
  private readonly aspectMode: AspectMode;
  private readonly display: DisplayGeometry;

  constructor(targetWidth: number, targetHeight: number, aspectMode: AspectMode, display: DisplayGeometry) {
    this.targetWidth = targetWidth;
    this.targetHeight = targetHeight;
    this.aspectMode = aspectMode;
    this.display = display;
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
      this.display,
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
