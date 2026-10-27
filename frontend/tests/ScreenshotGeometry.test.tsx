import { expect, it } from "vitest";
import { screenshotGeometry } from "../src/screenshotGeometry";

it.each(["content-box", "border-box"])("preserves fractional spacer border boxes with %s sizing", boxSizing => {
  const geometry = screenshotGeometry(new AbortController().signal);
  const source = document.createElement("p");
  source.style.cssText = "width:120.25px;height:29.75px;padding:1.5px 2.25px;border:1px solid;box-sizing:" + boxSizing + ";transform:scale(2)";
  source.textContent = "前文段落";
  geometry.track(source);
  const spacer = source.cloneNode(false) as HTMLElement;
  spacer.style.cssText = "visibility:hidden;box-sizing:border-box;width:127px;min-width:127px;max-width:127px;height:35px;min-height:35px;max-height:35px";
  geometry.restore(spacer);
  const width = boxSizing === "border-box" ? "120.25px" : "126.75px";
  const height = boxSizing === "border-box" ? "29.75px" : "34.75px";
  for (const prefix of ["", "min-", "max-"]) {
    expect(spacer.style.getPropertyValue(prefix + "width")).toBe(width);
    expect(spacer.style.getPropertyValue(prefix + "height")).toBe(height);
  }
  expect(Array.from(spacer.attributes).some(attr => attr.name.startsWith("data-reader-geometry-"))).toBe(false);
  geometry.release();
  expect(source.outerHTML).not.toContain("data-reader-geometry-");
  expect(source.style.height).toBe("29.75px");
});

it("leaves visible clones and intentionally hidden publication content alone", () => {
  const geometry = screenshotGeometry(new AbortController().signal);
  const root = document.createElement("section");
  root.innerHTML = '<p style="height:29.75px">正文</p><p style="visibility:hidden;height:29.75px">隐藏正文</p>';
  for (const source of root.children) geometry.track(source);
  const clone = root.cloneNode(true) as HTMLElement;
  const hidden = clone.lastElementChild as HTMLElement;
  hidden.textContent = ""; hidden.style.height = "30px";
  geometry.restore(clone);
  expect((clone.firstElementChild as HTMLElement).style.height).toBe("29.75px");
  expect(hidden.style.height).toBe("30px");
  geometry.release();
  expect(root.outerHTML).not.toContain("data-reader-geometry-");
});

it("releases temporary source markers after cancellation and rejects further tracking", () => {
  const controller = new AbortController();
  const geometry = screenshotGeometry(controller.signal);
  const source = document.createElement("p");
  geometry.track(source);
  controller.abort(new Error("cancelled"));
  geometry.release();
  expect(source.attributes).toHaveLength(0);
  expect(() => geometry.track(source)).toThrow("cancelled");
  expect(source.attributes).toHaveLength(0);
});
