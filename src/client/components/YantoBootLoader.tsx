import { useEffect, useState } from "react";
import { drawYantoCore } from "../ascii/drawYantoCore";
import type { PlotOptions } from "../ascii/types";
import { LoadingInline } from "./ui";

const asciiWidth = 74;
const asciiHeight = 24;

const bufferSize = asciiWidth * asciiHeight;
const chars: string[] = new Array(bufferSize);
const zBuffer: number[] = new Array(bufferSize);

function renderFrame(time: number) {
  for (let i = 0; i < bufferSize; i++) {
    chars[i] = " ";
    zBuffer[i] = -Infinity;
  }
  const options: PlotOptions = {
    width: asciiWidth,
    height: asciiHeight,
    chars,
    zBuffer,
    scaleX: 9.6,
    scaleY: 5.4,
  };

  drawYantoCore(time, options);

  const lines: string[] = [];
  for (let row = 0; row < asciiHeight; row += 1) {
    lines.push(chars.slice(row * asciiWidth, (row + 1) * asciiWidth).join("").trimEnd());
  }

  return lines.join("\n");
}

export function YantoBootLoader() {
  const [frame, setFrame] = useState(() => renderFrame(0));

  useEffect(() => {
    let animationFrame = 0;
    let lastPaint = 0;

    const animate = (time: number) => {
      if (time - lastPaint > 33) {
        setFrame(renderFrame(time));
        lastPaint = time;
      }

      animationFrame = window.requestAnimationFrame(animate);
    };

    animationFrame = window.requestAnimationFrame(animate);
    return () => window.cancelAnimationFrame(animationFrame);
  }, []);

  return (
    <section className="boot-loader" aria-label="Starting Yanto">
      <pre className="boot-loader__ascii" aria-hidden="true">{frame}</pre>
      <LoadingInline label="Starting Yanto" />
    </section>
  );
}
