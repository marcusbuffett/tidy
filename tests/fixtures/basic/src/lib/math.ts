export function add(a: number, b: number) { return a + b; }
export function subtract(a: number, b: number) { return a - b; }
export type Point = { x: number; y: number };
export type Vector = Point;
export interface Shape { area(): number }
/** @public */
export const PUBLIC_API = 42;
