import { calculateCompositorLayout, canPassThroughFrame } from './compositor.ts';

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

function assertRect(actual: number[], expected: number[], label: string): void {
  assert(actual.length === expected.length && actual.every((value, index) => value === expected[index]), `${label}: ${actual} !== ${expected}`);
}

const sourceWidth = 3456;
const sourceHeight = 2234;
const display = {
  signalWidth: 3840,
  signalHeight: 2160,
  panelWidth: 3840,
  panelHeight: 2400,
};

const preserve = calculateCompositorLayout(
  sourceWidth,
  sourceHeight,
  1920,
  1080,
  'preserve',
  display,
);
assertRect(
  [preserve.contentX, preserve.contentY, preserve.contentWidth, preserve.contentHeight],
  [31, 0, 1857, 1080],
  'preserve encoded rectangle',
);
assert(Math.abs(
  preserve.panelContentWidth / preserve.panelContentHeight - sourceWidth / sourceHeight,
 ) < 0.001, 'preserve panel layout keeps the source aspect');
assertRect(
  [preserve.signalContentX, preserve.signalContentY, preserve.signalContentWidth, preserve.signalContentHeight],
  [63, 0, 3713, 2160],
  'preserve signal rectangle',
);
assertRect(
  [preserve.panelContentX, preserve.panelContentY, preserve.panelContentWidth, preserve.panelContentHeight],
  [63, 0, 3713, 2400],
  'preserve panel rectangle',
);

const stretch = calculateCompositorLayout(
  sourceWidth,
  sourceHeight,
  1920,
  1080,
  'stretch',
  display,
);
assertRect(
  [stretch.contentX, stretch.contentY, stretch.contentWidth, stretch.contentHeight],
  [0, 0, 1920, 1080],
  'stretch encoded rectangle',
);
assertRect(
  [stretch.signalContentX, stretch.signalContentY, stretch.signalContentWidth, stretch.signalContentHeight],
  [0, 0, 3840, 2160],
  'stretch signal rectangle',
);
assert(Math.abs(stretch.contentWidth / stretch.contentHeight - display.signalWidth / display.signalHeight) < 0.001, 'stretch does not fill the signal aspect');
assert(Math.abs(
  stretch.panelContentWidth / stretch.panelContentHeight
    - preserve.panelContentWidth / preserve.panelContentHeight,
) > 0.01, 'preserve and stretch use distinct panel layouts');

const full4kPreserve = calculateCompositorLayout(3840, 2160, 3840, 2160, 'preserve', display);
assert(!canPassThroughFrame(full4kPreserve), 'matching-size Preserve frames still compose when panel compensation is required');
const full4kStretch = calculateCompositorLayout(3840, 2160, 3840, 2160, 'stretch', display);
assert(canPassThroughFrame(full4kStretch), 'matching-size Stretch frames use the pass-through path');
const live4kPreserve = calculateCompositorLayout(sourceWidth, sourceHeight, 3840, 2160, 'preserve', display);
assertRect(
  [live4kPreserve.contentX, live4kPreserve.contentY, live4kPreserve.contentWidth, live4kPreserve.contentHeight],
  [63, 0, 3713, 2160],
  'live Mac capture into 4K Preserve canvas',
);

for (const layout of [preserve, stretch, full4kPreserve, full4kStretch, live4kPreserve]) {
  assert(layout.contentX >= 0 && layout.contentY >= 0, 'content starts inside the encoder canvas');
  assert(layout.contentX + layout.contentWidth <= layout.targetWidth, 'content does not cross the right canvas edge');
  assert(layout.contentY + layout.contentHeight <= layout.targetHeight, 'content does not cross the bottom canvas edge');
  assert(Math.abs(layout.contentX - (layout.targetWidth - layout.contentX - layout.contentWidth)) <= 1, 'horizontal bars are symmetric');
  assert(Math.abs(layout.contentY - (layout.targetHeight - layout.contentY - layout.contentHeight)) <= 1, 'vertical bars are symmetric');
}

console.log('compositor aspect tests passed');
