import { plot, rotate } from "./asciiRenderer";
import type { PlotOptions } from "./types";

export function drawYantoCore(time: number, options: PlotOptions) {
  const phase = time * 0.001;
  const name = "YANTO";
  const letterWidth = 5;
  const letterGap = 1;
  const totalColumns = name.length * letterWidth + (name.length - 1) * letterGap;
  const spin = Math.sin(phase * 1.2) * 0.07;

  for (let i = 0; i < 128; i += 1) {
    const angle = (i / 128) * Math.PI * 2 + phase * 0.7;
    const point = rotate(
      {
        x: Math.cos(angle) * 2.95,
        y: Math.sin(angle * 2 + phase) * 0.18,
        z: Math.sin(angle) * 0.9 + 0.72,
      },
      0.98,
      phase * 0.5,
      0,
    );

    plot(point, i % 34 === 0 ? "*" : ".", options);
  }

  for (let spark = 0; spark < 10; spark += 1) {
    const seed = spark * 12.9898;
    const drift = phase * (0.32 + (spark % 5) * 0.04);
    const point = rotate(
      {
        x: Math.sin(seed + drift) * 3.1,
        y: Math.cos(seed * 0.72 - drift) * 1.7,
        z: Math.sin(seed * 1.37 + phase) * 1.2 + 0.35,
      },
      0.28,
      phase * 0.24,
      -0.18,
    );

    if (Math.sin(phase * 5 + spark) > 0.35) {
      plot(point, "*", options);
    }
  }

  for (let letterIndex = 0; letterIndex < name.length; letterIndex += 1) {
    const letter = name[letterIndex];
    const glyph = letter ? yantoFont[letter] : undefined;
    if (!glyph) continue;

    for (let row = 0; row < glyph.length; row += 1) {
      const line = glyph[row];
      if (!line) continue;

      for (let col = 0; col < line.length; col += 1) {
        if (line[col] !== "1") continue;

        const column = letterIndex * (letterWidth + letterGap) + col;
        const baseX = (column - totalColumns / 2) * 0.2;
        const baseY = (3 - row) * 0.34;

        for (let layer = 0; layer < 4; layer += 1) {
          for (let fillX = 0; fillX < 5; fillX += 1) {
            for (let fillY = 0; fillY < 3; fillY += 1) {
              const x = baseX + (fillX - 2) * 0.052;
              const y = baseY + (fillY - 1) * 0.07;
              const z = -1.05 + layer * 0.1;
              const wave = Math.sin(phase * 4 + column * 0.6 + row) * 0.04;
              const point = rotate(
                {
                  x,
                  y: y + wave,
                  z,
                },
                -0.06 + Math.sin(phase * 0.9) * 0.04,
                spin,
                Math.sin(phase * 0.7) * 0.025,
              );
              const char = layer < 2 ? letter : "#";

              plot(point, char, options);
            }
          }
        }
      }
    }
  }
}

const yantoFont: Record<string, string[]> = {
  Y: ["10001", "01010", "00100", "00100", "00100", "00100", "00100"],
  A: ["01110", "10001", "10001", "11111", "10001", "10001", "10001"],
  N: ["10001", "11001", "10101", "10011", "10001", "10001", "10001"],
  T: ["11111", "00100", "00100", "00100", "00100", "00100", "00100"],
  O: ["01110", "10001", "10001", "10001", "10001", "10001", "01110"],
};
