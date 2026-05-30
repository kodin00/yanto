import type { PlotOptions, Point3D } from "./types";

export function rotate(point: Point3D, pitch: number, yaw: number, roll: number): Point3D {
  const cosPitch = Math.cos(pitch);
  const sinPitch = Math.sin(pitch);
  const cosYaw = Math.cos(yaw);
  const sinYaw = Math.sin(yaw);
  const cosRoll = Math.cos(roll);
  const sinRoll = Math.sin(roll);

  const pitched = {
    x: point.x,
    y: point.y * cosPitch - point.z * sinPitch,
    z: point.y * sinPitch + point.z * cosPitch,
  };

  const yawed = {
    x: pitched.x * cosYaw + pitched.z * sinYaw,
    y: pitched.y,
    z: -pitched.x * sinYaw + pitched.z * cosYaw,
  };

  return {
    x: yawed.x * cosRoll - yawed.y * sinRoll,
    y: yawed.x * sinRoll + yawed.y * cosRoll,
    z: yawed.z,
  };
}

export function plot(point: Point3D, char: string, options: PlotOptions) {
  const depth = 5 + point.z;
  if (depth <= 0.1) return;

  const perspective = 5 / depth;
  const x = Math.round(options.width / 2 + point.x * options.scaleX * perspective);
  const y = Math.round(options.height / 2 - point.y * options.scaleY * perspective);

  if (x < 0 || x >= options.width || y < 0 || y >= options.height) return;

  const index = y * options.width + x;
  if (perspective <= options.zBuffer[index]) return;

  options.zBuffer[index] = perspective;
  options.chars[index] = char;
}
