const value = require('./value');

it('should invalidate one loader when its input changes', () => {
  expect(value).toEqual({
    value: +WATCH_STEP < 3 ? 'initial' : 'changed',
    leftRuns: +WATCH_STEP + 1,
    markedRuns: +WATCH_STEP + 1,
    rightRuns: +WATCH_STEP + 1,
    sourceMap: true,
    additionalData: true,
  });
});
