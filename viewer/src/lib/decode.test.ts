// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { sizeCanvas } from "./decode";

describe("sizeCanvas", () => {
  it("leaves the canvas alone when the size is unchanged", () => {
    const canvas = document.createElement("canvas");
    canvas.width = 640;
    canvas.height = 480;
    const setWidth = vi.spyOn(canvas, "width", "set");
    const setHeight = vi.spyOn(canvas, "height", "set");

    sizeCanvas(canvas, 640, 480);

    expect(setWidth).not.toHaveBeenCalled();
    expect(setHeight).not.toHaveBeenCalled();
  });

  it("resizes when the remote screen size changes", () => {
    const canvas = document.createElement("canvas");
    sizeCanvas(canvas, 1920, 1080);
    expect(canvas.width).toBe(1920);
    expect(canvas.height).toBe(1080);
  });
});
