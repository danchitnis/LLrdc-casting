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
  [32, 0, 1857, 1080],
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

assert(canPassThroughFrame(1920, 1088, 1920, 1088), 'codec-aligned frames use the pass-through path');
assert(!canPassThroughFrame(1920, 1080, 1920, 1088), 'unaligned 1080p frames still require composition');

console.log('compositor aspect tests passed');
