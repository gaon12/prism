import { calculate, convertUnits } from './utility-engine';
self.postMessage({ ready: true });
self.onmessage = (
  event: MessageEvent<{
    input: string;
    kind: 'calculator' | 'units';
    precision: number;
    angle: 'rad' | 'deg';
  }>,
) => {
  try {
    const { input, kind, precision, angle } = event.data;
    self.postMessage({
      answer:
        kind === 'calculator' ? calculate(input, precision, angle) : convertUnits(input, precision),
    });
  } catch {
    self.postMessage({ error: true });
  }
};
