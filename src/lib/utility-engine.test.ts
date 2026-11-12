import { describe, expect, it } from 'vitest';
import { calculate, convertUnits } from './utility-engine';
import { parseInput, webSearchUrl } from './utilities';

describe('explicit utility routing', () => {
  it('keeps filenames, drive letters and filters in file search', () => {
    for (const query of ['2+3.txt', 'C:\\Documents\\계획', 'extension:pdf 계획', '2026-11-04'])
      expect(parseInput(query).kind).toBe('files');
    expect(parseInput('= 2+3')).toEqual({ kind: 'calculator', input: '2+3' });
    expect(parseInput('unit: 10인치 to cm').kind).toBe('units');
    expect(parseInput('wiki: 한글').provider).toBe('wiki');
    expect(parseInput('clip: ㄱㅁㅅ')).toEqual({ kind: 'clipboard', input: 'ㄱㅁㅅ' });
    expect(parseInput('클립보드:')).toEqual({ kind: 'clipboard', input: '' });
    expect(parseInput('dict:apple')).toEqual({ kind: 'dictionary', input: 'apple' });
    expect(parseInput('사전:ja りんご')).toEqual({
      kind: 'dictionary',
      input: 'りんご',
      dictionaryLanguage: 'ja',
    });
    expect(parseInput('dict:ko')).toEqual({
      kind: 'dictionary',
      input: '',
      dictionaryLanguage: 'ko',
    });
  });
  it('encodes web queries without interpreting them as URLs or extra parameters', () => {
    const url = new URL(webSearchUrl('google', '한글 & a=1 #topic'));
    expect(url.protocol).toBe('https:');
    expect([...url.searchParams]).toEqual([['q', '한글 & a=1 #topic']]);
  });
});
describe('general calculator', () => {
  it.each([
    ['48/2(9+3)', 2],
    ['48÷2(9+3)', 2],
    ['48/2*(9+3)', 288],
    ['(48/2)(9+3)', 288],
    ['8/2(2)(2)', 1],
    ['48/2/3(4+5)', 8 / 9],
    ['48*2/3(4+5)', 32 / 9],
    ['2pi/2pi', 1],
    ['12/2pi*2', 12 / Math.PI],
    ['2(3)^2', 18],
    ['-48/2(9+3)', -2],
    ['8/2(2+2)+1', 2],
  ])('binds implicit multiplication first in %s', (input, expected) =>
    expect(Number(calculate(input).value)).toBeCloseTo(expected, 10),
  );
  it('shows the grouped interpretation without changing explicit parentheses', () => {
    const result = calculate('48/2(9+3)');
    expect(result.interpretation).toBe('48 / (2 * (9 + 3))');
    expect(calculate('48/(2*(9+3))').value).toBe('2');
    expect(calculate('48/2*(9+3)').interpretation).toBeUndefined();
  });
  it('honors precedence, exact fractions, decimals, roots and angle settings', () => {
    expect(calculate('2+3*4').value).toBe('14');
    expect(calculate('0.1+0.2').value).toBe('0.3');
    expect(calculate('1/3')).toMatchObject({
      value: '0.333333333333',
      exact: '1/3',
      accuracy: { significantDigits: 12, rounded: true },
    });
    expect(Number(calculate('sqrt(2)').value)).toBeCloseTo(Math.SQRT2, 11);
    expect(calculate('sin(pi/2)').value).toBe('1');
    expect(calculate('sin(90)', 12, 'deg').value).toBe('1');
    expect(calculate('log10(100)').value).toBe('2');
    expect(calculate('5!').value).toBe('120');
    expect(calculate('50%').value).toBe('0.5');
  });
  it('rejects assignments, access, collections, unknown functions and undefined real values', () => {
    for (const expression of [
      'x=2',
      'import("x")',
      '[1,2]',
      'a.b',
      '1/0',
      'sqrt(-1)',
      '1001!',
      '2+',
      'sin(2,3)',
    ])
      expect(() => calculate(expression), expression).toThrow();
  });
  it('preserves finite large and small CortexJS results without machine-number overflow', () => {
    expect(calculate('10^400').value).toBe('1e+400');
    expect(calculate('10^-400').value).toBe('1e-400');
    expect(calculate('1000!').value).toMatch(/^4\.02387260077e\+2567$/);
  });
  it('preserves numeric literals before evaluation and display rounding', () => {
    expect(calculate('9007199254740993 - 9007199254740992').value).toBe('1');
    expect(calculate('1.0000000000000001 - 1').value).toBe('1e-16');
    expect(calculate('1e400 / 1e400').value).toBe('1');
    expect(calculate('1e-400 / 1e-400').value).toBe('1');
  });
  it('distinguishes actual rounding from exact values and scientific formatting', () => {
    for (const input of ['2', '1/8', '0.1+0.2', '10^400'])
      expect(calculate(input).accuracy, input).toBeUndefined();
    expect(calculate('1/3', 6).accuracy).toEqual({ significantDigits: 6, rounded: true });
    expect(calculate('sqrt(2)').accuracy?.rounded).toBe(true);
    expect(calculate('123456789', 6)).toMatchObject({
      value: '123457000',
      accuracy: { significantDigits: 6, rounded: true },
    });
    expect(convertUnits('20 km/L to L/100km').accuracy).toBeUndefined();
    expect(convertUnits('30 mpg to L/100km').accuracy?.rounded).toBe(true);
  });
});
describe('ten unit categories', () => {
  it.each([
    ['10인치 to cm', '25.4 cm'],
    ['1 m² to cm2', '10000 cm²'],
    ['1 kg to g', '1000 g'],
    ['1 L to mL', '1000 mL'],
    ['0 섭씨 to 화씨', '32 °F'],
    ['1 atm to Pa', '101325 Pa'],
    ['36 km/h to m/s', '10 m/s'],
    ['20 km/L to L/100km', '5 L/100km'],
    ['1 GB to MB', '1000 MB'],
    ['1 GiB to MiB', '1024 MiB'],
    ['1시간 to min', '60 min'],
  ])('%s → %s', (input, expected) => expect(convertUnits(input).value).toBe(expected));
  it('handles reciprocal US fuel economy and rejects incompatible or impossible values', () => {
    expect(Number(convertUnits('30 mpg to L/100km').value.split(' ')[0])).toBeCloseTo(
      7.84048611111,
      9,
    );
    for (const input of [
      '1 m to kg',
      '0 km/L to mpg',
      '-274 C to K',
      '-2 kg to g',
      '1 unknown to m',
      '10 inch',
    ])
      expect(() => convertUnits(input)).toThrow();
  });
});
