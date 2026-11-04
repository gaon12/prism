import { ComputeEngine, isNumber } from '@cortex-js/compute-engine/core';
import type { MathJsonExpression as Expression } from '@cortex-js/compute-engine/math-json';
import { parse, unit, format, bignumber, OperatorNode } from 'mathjs';
import type { MathNode } from 'mathjs';
import type { UtilityAnswer } from './utilities';

const engine = new ComputeEngine({ precision: 32 });
const operators: Record<string, string> = {
  add: 'Add',
  subtract: 'Subtract',
  multiply: 'Multiply',
  divide: 'Divide',
  pow: 'Power',
  unaryMinus: 'Negate',
  unaryPlus: 'Identity',
  factorial: 'Factorial',
  mod: 'Mod',
};
const functions: Record<string, string> = {
  sqrt: 'Sqrt',
  abs: 'Abs',
  sin: 'Sin',
  cos: 'Cos',
  tan: 'Tan',
  asin: 'Arcsin',
  acos: 'Arccos',
  atan: 'Arctan',
  log: 'Ln',
  ln: 'Ln',
  log10: 'Log10',
  exp: 'Exp',
  floor: 'Floor',
  ceil: 'Ceil',
  round: 'Round',
};
type Ast = MathNode & {
  args?: Ast[];
  content?: Ast;
  value?: unknown;
  name?: string;
  fn?: string | { name?: string };
  op?: string;
  implicit?: boolean;
  isPercentage?: boolean;
};
function implicitProductsFirst(node: Ast): Ast {
  const mapped = node.map((child) => implicitProductsFirst(child as Ast)) as Ast;
  const isProduct = (item: Ast) =>
    item.type === 'OperatorNode' &&
    ['multiply', 'divide'].includes(String(item.fn)) &&
    item.args?.length === 2 &&
    !item.isPercentage;
  if (!isProduct(mapped)) return mapped;
  const values: Ast[] = [];
  const operations: { divide: boolean; implicit: boolean }[] = [];
  const flatten = (item: Ast) => {
    if (!isProduct(item)) {
      values.push(item);
      return;
    }
    flatten(item.args![0]);
    operations.push({ divide: item.fn === 'divide', implicit: item.implicit === true });
    flatten(item.args![1]);
  };
  flatten(mapped);
  // Rebuild each unparenthesized product/division chain with juxtaposition first.
  // Parenthesis, functions, powers and percentage nodes stay atomic.
  const groups: Ast[] = [];
  const explicit: boolean[] = [];
  let group = values[0];
  for (const [index, operation] of operations.entries()) {
    if (!operation.divide && operation.implicit)
      group = new OperatorNode('*', 'multiply', [group, values[index + 1]], true) as Ast;
    else {
      groups.push(group);
      explicit.push(operation.divide);
      group = values[index + 1];
    }
  }
  groups.push(group);
  return groups
    .slice(1)
    .reduce(
      (left, right, index) =>
        new OperatorNode(explicit[index] ? '/' : '*', explicit[index] ? 'divide' : 'multiply', [
          left,
          right,
        ]) as Ast,
      groups[0],
    );
}
function mathJson(node: Ast, angle: 'rad' | 'deg', depth = 0): Expression {
  if (depth > 24) throw new Error('INVALID_EXPRESSION');
  const convert = (child: Ast) => mathJson(child, angle, depth + 1);
  if (node.type === 'ConstantNode' && typeof node.value === 'number' && Number.isFinite(node.value))
    return node.value;
  if (node.type === 'ParenthesisNode' && node.content) return convert(node.content);
  if (node.type === 'SymbolNode' && ['pi', 'e'].includes(node.name ?? ''))
    return node.name === 'pi' ? 'Pi' : 'ExponentialE';
  if (node.type === 'OperatorNode' && typeof node.fn === 'string' && operators[node.fn]) {
    const args = node.args?.map(convert) ?? [];
    if (
      node.fn === 'factorial' &&
      (typeof args[0] !== 'number' || args[0] < 0 || args[0] > 1000 || !Number.isInteger(args[0]))
    )
      throw new Error('INVALID_EXPRESSION');
    if (node.fn === 'unaryPlus') return args[0];
    return [operators[node.fn], ...args];
  }
  if (
    node.type === 'FunctionNode' &&
    typeof node.fn !== 'string' &&
    node.fn?.name &&
    functions[node.fn.name]
  ) {
    const fn = node.fn.name;
    if (node.args?.length !== 1) throw new Error('INVALID_EXPRESSION');
    let arg = convert(node.args[0]);
    if (angle === 'deg' && ['sin', 'cos', 'tan'].includes(fn))
      arg = ['Multiply', arg, ['Divide', 'Pi', 180]];
    const result: Expression = [functions[fn], arg];
    return angle === 'deg' && ['asin', 'acos', 'atan'].includes(fn)
      ? ['Multiply', result, ['Divide', 180, 'Pi']]
      : result;
  }
  throw new Error('INVALID_EXPRESSION');
}
function numberText(value: number, precision: number) {
  if (!Number.isFinite(value)) throw new Error('INVALID_EXPRESSION');
  return format(Object.is(value, -0) ? 0 : value, {
    precision: Math.max(6, Math.min(15, precision)),
    lowerExp: -9,
    upperExp: 15,
  });
}
export function calculate(
  input: string,
  precision = 12,
  angle: 'rad' | 'deg' = 'rad',
): UtilityAnswer {
  if (!input || input.length > 256) throw new Error('INVALID_EXPRESSION');
  const clean = input
    .replaceAll('×', '*')
    .replaceAll('÷', '/')
    .replaceAll('−', '-')
    .replaceAll('π', 'pi');
  const tree = parse(clean) as Ast;
  let count = 0;
  let implicit = false;
  tree.traverse(() => {
    if (++count > 128) throw new Error('INVALID_EXPRESSION');
  });
  tree.traverse((node) => {
    if ((node as Ast).implicit) implicit = true;
  });
  const interpreted = implicitProductsFirst(tree);
  // Only the numeric AST above crosses into CortexJS; no assignments, accessors, code or collections.
  const exact = engine.expr(mathJson(interpreted, angle)).evaluate();
  const approximate = exact.N().toString();
  const numeric = bignumber(approximate);
  if (!numeric.isFinite()) throw new Error('INVALID_EXPRESSION');
  const value = format(numeric.isZero() ? bignumber(0) : numeric, {
    precision: Math.max(6, Math.min(15, precision)),
    lowerExp: -9,
    upperExp: 15,
  });
  const exactText = exact.toString();
  const rounded = !numeric.eq(bignumber(value));
  const needsApproximation = rounded || !isNumber(exact);
  return {
    value,
    ...(needsApproximation
      ? { accuracy: { significantDigits: Math.max(6, Math.min(15, precision)), rounded } }
      : {}),
    ...(exactText !== value && exactText.length < 100 ? { exact: exactText } : {}),
    ...(implicit
      ? { interpretation: interpreted.toString({ parenthesis: 'all', implicit: 'show' }) }
      : {}),
  };
}

type UnitSpec = { canonical: string; category: string };
const aliases = new Map<string, UnitSpec>();
function units(category: string, entries: Record<string, string[]>) {
  for (const [canonical, names] of Object.entries(entries))
    for (const name of [canonical, ...names]) aliases.set(name, { canonical, category });
}
units('length', {
  m: ['미터'],
  cm: ['센티미터'],
  mm: ['밀리미터'],
  km: ['킬로미터'],
  inch: ['in', '인치'],
  ft: ['피트', 'feet'],
  yd: ['야드'],
  mile: ['mi', '마일'],
});
units('area', {
  'm^2': ['m2', 'm²', '제곱미터'],
  'cm^2': ['cm2', 'cm²'],
  'km^2': ['km2', 'km²'],
  'ft^2': ['ft2', 'ft²'],
  ha: ['헥타르'],
  acre: ['에이커'],
});
units('mass', {
  kg: ['킬로그램'],
  g: ['그램'],
  mg: ['밀리그램'],
  lb: ['파운드', 'lbs'],
  oz: ['온스'],
  tonne: ['t', '톤'],
});
units('volume', {
  L: ['l', '리터'],
  mL: ['ml', '밀리리터'],
  'm^3': ['m3', 'm³', '세제곱미터'],
  gallon: ['gal', '갤런'],
  cup: ['컵'],
});
units('temperature', { degC: ['C', '°C', '섭씨'], degF: ['F', '°F', '화씨'], K: ['켈빈'] });
units('pressure', { Pa: ['파스칼'], kPa: [], MPa: [], bar: ['바'], atm: ['기압'], psi: [] });
units('speed', { 'km/h': ['kph', '시속'], 'm/s': ['mps', '초속'], 'mile/h': ['mph'] });
units('fuel', { 'km/L': ['km/l'], 'L/100km': ['l/100km'], mpg: ['mpg-US'] });
units('data', {
  B: ['byte', '바이트'],
  kB: ['KB'],
  MB: [],
  GB: [],
  TB: [],
  KiB: [],
  MiB: [],
  GiB: [],
  TiB: [],
  b: ['bit', '비트'],
  kb: [],
  Mb: [],
  Gb: [],
});
units('time', {
  ms: ['밀리초'],
  s: ['sec', '초'],
  min: ['분'],
  h: ['hr', '시간'],
  day: ['일'],
  week: ['주'],
});
export function convertUnits(input: string, precision = 12): UtilityAnswer {
  if (input.length > 256) throw new Error('INVALID_UNIT');
  const match =
    /^\s*([+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?)\s*(.+?)\s*(?:\s+to\s+|\s*→\s*|\s*->\s*)(.+?)\s*$/i.exec(
      input,
    );
  if (!match) throw new Error('INVALID_UNIT');
  const value = Number(match[1]);
  const from = aliases.get(match[2].trim());
  const to = aliases.get(match[3].trim());
  if (!Number.isFinite(value) || !from || !to || from.category !== to.category)
    throw new Error('INVALID_UNIT');
  let result: number;
  if (from.category === 'fuel') {
    if (value <= 0) throw new Error('INVALID_UNIT');
    const kmPerLitre =
      from.canonical === 'L/100km'
        ? 100 / value
        : from.canonical === 'mpg'
          ? (value * 1.609344) / 3.785411784
          : value;
    result =
      to.canonical === 'L/100km'
        ? 100 / kmPerLitre
        : to.canonical === 'mpg'
          ? (kmPerLitre * 3.785411784) / 1.609344
          : kmPerLitre;
  } else {
    if (from.category !== 'temperature' && value < 0) throw new Error('INVALID_UNIT');
    if (from.category === 'temperature' && unit(value, from.canonical).toNumber('K') < 0)
      throw new Error('INVALID_UNIT');
    result = unit(value, from.canonical).toNumber(to.canonical);
  }
  return {
    value: `${numberText(result, precision)} ${to.canonical === 'mpg' ? 'mpg (US)' : to.canonical.replace('deg', '°').replace('^2', '²').replace('^3', '³')}`,
    ...(bignumber(result).eq(bignumber(numberText(result, precision)))
      ? {}
      : { accuracy: { significantDigits: Math.max(6, Math.min(15, precision)), rounded: true } }),
  };
}
