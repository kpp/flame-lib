import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const html = readFileSync(new URL("./index.html", import.meta.url), "utf8");
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];

// Exercise the shipped script with a tiny DOM fixture; this is not a layout test.
function app() {
  const elements = new Map();
  let animation = null;
  for (const match of html.matchAll(/\bid="([\w-]+)"/g)) {
    const element = {
      value: "", hidden: false, textContent: "", innerHTML: "",
      width: 960, height: 350, attributes: {}, listeners: {},
      addEventListener(type, listener) { this.listeners[type] = listener; },
      setAttribute(name, value) { this.attributes[name] = String(value); },
      toggleAttribute(name, enabled) { this[name] = enabled; },
      getBoundingClientRect() { return { width: this.width, height: this.height, left: 0 }; },
    };
    elements.set(match[1], element);
  }
  for (const match of html.matchAll(/<input\b[^>]*id="([\w-]+)"[^>]*value="([^"]*)"/g)) elements.get(match[1]).value = match[2];
  elements.get("trend").value = "linear";
  const context = vm.createContext({
    document: { getElementById(id) { assert(elements.has(id), `Unknown element: ${id}`); return elements.get(id); } },
    requestAnimationFrame(callback) { animation = callback; return 1; },
    ResizeObserver: class { observe() {} },
    Intl,
  });
  vm.runInContext(script + "\nglobalThis.demo = { logValue, marketRates, simulate, rewardsByDuration, redraw, noisePath: NOISE_PATH, getState: () => state };", context);
  return {
    ...context.demo,
    get: id => elements.get(id),
    update(values) {
      for (const [id, value] of Object.entries(values)) elements.get(id).value = String(value);
      elements.get("controls").listeners.input();
      assert(animation, "Control changes must schedule a redraw");
      const next = animation;
      animation = null;
      next();
    },
  };
}

function close(actual, expected) {
  assert(Math.abs(actual - expected) <= 1e-10 * Math.max(1, Math.abs(expected)), `${actual} != ${expected}`);
}

const { logValue, marketRates, simulate, rewardsByDuration, noisePath } = app();

test("noise is reproducible, stays positive and zero reproduces the old curves exactly", () => {
  for (const trend of ["linear", "quadratic", "exponential", "sqrt"]) {
    for (const slope of [-95, 900]) {
      const smooth = marketRates(trend, slope);
      assert.deepEqual(marketRates(trend, slope, 0), smooth);
      const noisy = marketRates(trend, slope, 30);
      assert.deepEqual(noisy, marketRates(trend, slope, 30));
      assert(noisy.every(rate => Number.isFinite(rate) && rate > 0));
      assert(noisy.some((rate, index) => rate !== smooth[index]));
    }
  }
  for (const noise of [-1, 31, NaN, Infinity]) assert.throws(() => marketRates("linear", 100, noise));
});

test("noise percentage calibrates relative standard deviation with correlated Gaussian innovations", () => {
  const base = marketRates("linear", 0);
  for (const noise of [1, 15, 30]) {
    const variance = Math.log1p((noise / 100) ** 2);
    const sigma = Math.sqrt(variance);
    close(Math.sqrt(Math.expm1(variance)), noise / 100);
    const rates = marketRates("linear", 0, noise);
    for (const index of [0, 1, 100, 5000, 9999]) close(rates[index] / base[index], Math.exp(sigma * noisePath[index] - variance / 2));
  }
  const correlation = Math.exp(-1 / 100);
  const scale = Math.sqrt(1 - correlation * correlation);
  const innovations = noisePath.slice(1).map((value, index) => (value - correlation * noisePath[index]) / scale);
  const mean = innovations.reduce((sum, value) => sum + value, 0) / innovations.length;
  const variance = innovations.reduce((sum, value) => sum + (value - mean) ** 2, 0) / innovations.length;
  assert(Math.abs(mean) < 0.05);
  assert(variance > 0.9 && variance < 1.1);
  const neighborChange = noisePath.slice(1).reduce((sum, value, index) => sum + (value - noisePath[index]) ** 2, 0) / (noisePath.length - 1);
  assert(neighborChange < 0.05, "Neighboring blocks should be correlated, not independent noise");
});

test("duration curve covers every fitting integer and redistributes the entire budget", () => {
  const rates = Array(10000).fill(100);
  for (const start of [0, 5000, 9000, 9900]) {
    const points = rewardsByDuration(rates, 1000000, start);
    const last = Math.min(5000, 10000 - start);
    assert.equal(points.length, last - 99);
    points.forEach((point, index) => {
      assert.equal(point.duration, 100 + index);
      close(point.reward, 1000000 / (100 + 1000000 / (50 * point.duration)));
      assert(point.reward > 0 && point.reward <= 50 * point.duration);
    });
  }
  assert.throws(() => rewardsByDuration(rates, 1000, 9901));
  assert.throws(() => rewardsByDuration(rates, -1, 0));
});

test("duration curve matches the selected-window simulation across trends and both directions", () => {
  for (const trend of ["linear", "quadratic", "exponential", "sqrt"]) {
    for (const slope of [-95, 900]) {
      const rates = marketRates(trend, slope);
      const points = rewardsByDuration(rates, 100000, 1234);
      for (const duration of [100, 707, 2502, 5000]) close(points[duration - 100].reward, simulate(rates, 100000, duration, 1234).reward);
    }
  }
  assert(rewardsByDuration(Array(10000).fill(0), 0, 9900).every(point => point.reward === 0));
  close(rewardsByDuration(Array(10000).fill(0), 1000, 9900)[0].reward, 5000);
});

test("logarithmic sliders have exact endpoints, geometric midpoints and monotonic steps", () => {
  for (const [min, max] of [[1000, 10000000], [100, 5000]]) {
    assert.equal(logValue(0, min, max), min);
    assert.equal(logValue(1000, min, max), max);
    assert.equal(logValue(500, min, max), Math.round(Math.sqrt(min * max)));
    let previous = min;
    for (let position = 0; position <= 1000; position++) {
      const value = logValue(position, min, max);
      assert(Number.isInteger(value) && value >= previous && value <= max);
      previous = value;
    }
  }
  for (const position of [-1, 1001, NaN, Infinity]) assert.throws(() => logValue(position, 100, 5000));
});

test("strategy controls are sliders in budget, start, duration order", () => {
  const choices = html.split('<div class="choices">')[1].split('<p id="error"')[0];
  const inputs = [...choices.matchAll(/<input\b[^>]*id="([^"]+)"[^>]*type="([^"]+)"/g)];
  assert.deepEqual(inputs.map(match => [match[1], match[2]]), [["budget", "range"], ["start", "range"], ["duration", "range"]]);
});

test("all four trends have exact endpoints, positive finite rates and the chosen direction", () => {
  for (const trend of ["linear", "quadratic", "exponential", "sqrt"]) {
    for (const change of [-95, 0, 200, 900]) {
      const rates = marketRates(trend, change);
      assert.equal(rates.length, 10000);
      close(rates[0], 100);
      close(rates.at(-1), 100 * (1 + change / 100));
      rates.forEach((rate, index) => {
        assert(Number.isFinite(rate) && rate > 0);
        if (index) assert(change >= 0 ? rate >= rates[index - 1] : rate <= rates[index - 1]);
      });
    }
  }
});

test("constant market distributes the fixed issuance proportionally", () => {
  const result = simulate(Array(10000).fill(100), 100000, 10, 0);
  close(result.burn, 10000);
  close(result.reward, 1000 / 3);
  close(result.average, 300);
  close(result.burn * result.duration, result.budget);
});

test("average mint cost is budget divided by reward, not the arithmetic mean", () => {
  const result = simulate([1, 3], 100, 2, 0);
  close(result.reward, 37.5);
  close(result.average, 8 / 3);
});

test("zero budget, no competitors and small or large burns stay finite", () => {
  const rates = Array(10000).fill(100);
  assert.equal(simulate(rates, 0, 10000, 0).reward, 0);
  assert.equal(simulate(rates, 0, 1, 9999).average, null);
  close(simulate([0, 0], 1, 2, 0).reward, 100);
  close(simulate([0], 0, 1, 0).reward, 0);
  for (const budget of [1, Number.MAX_SAFE_INTEGER]) {
    for (const [duration, start] of [[10000, 0], [1, 9999]]) {
      const result = simulate(rates, budget, duration, start);
      assert(result.reward > 0 && result.reward <= 50 * duration);
      assert(Number.isFinite(result.average));
    }
  }
});

test("start and duration select exactly the intended blocks", () => {
  const rates = marketRates("linear", 200);
  const result = simulate(rates, 100, 1, 9999);
  close(result.reward, 100 / (300 + 2));
  assert.equal(result.end, 9999);
  close(simulate(rates, 100, 1, 0).reward, 100 / (100 + 2));
  assert(simulate(rates, 1000000, 100, 0).reward > simulate(rates, 1000000, 100, 9900).reward);
});

test("invalid values and intervals are rejected rather than silently truncating budget", () => {
  const rates = Array(10000).fill(100);
  for (const budget of [-1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) assert.throws(() => simulate(rates, budget, 10, 0));
  for (const duration of [0, -1, 10001, 1.5, NaN]) assert.throws(() => simulate(rates, 100, duration, 0));
  for (const start of [-1, 10000, 10001, 1.5, NaN]) assert.throws(() => simulate(rates, 100, 1, start));
  assert.throws(() => simulate(rates, 100, 2, 9999));
  assert.throws(() => simulate([NaN], 1, 1, 0));
  assert.throws(() => marketRates("unknown", 100));
  assert.throws(() => marketRates("linear", -100));
});

test("the shipped page initializes and every control updates the chart and results", () => {
  const page = app();
  assert(page.get("error").hidden);
  assert.match(page.get("reward").innerHTML, /FLM/);
  for (const trend of ["linear", "quadratic", "exponential", "sqrt"]) {
    page.update({ trend, slope: -95, budget: 500, duration: 0, start: 100 });
    assert(page.get("error").hidden);
    assert.match(page.get("allocation").textContent, /Blocks 100–199/);
    assert.equal(page.get("budget-value").textContent, "100,000");
    assert.equal(page.get("duration-value").textContent, "100");
    assert.equal(page.get("start-value").textContent, "100");
    assert.equal(page.get("budget").attributes["aria-valuetext"], "100,000 satoshis");
    assert.equal(page.get("duration").attributes["aria-valuetext"], "100 blocks");
    assert.match(page.get("chart").innerHTML, /class="market-path"/);
    assert.match(page.get("chart").innerHTML, /class="user-path"/);
    assert.doesNotMatch(page.get("chart").innerHTML, /NaN|Infinity|undefined/);
  }
  for (const width of [284, 600, 1200]) {
    page.get("chart").width = width;
    page.redraw();
    assert.equal(page.get("chart").attributes.viewBox, `0 0 ${width} 350`);
  }
});

test("budget slider endpoints and invalid-position recovery use the displayed values", () => {
  const page = app();
  page.update({ budget: 0 });
  assert.equal(page.get("budget-value").textContent, "1,000");
  assert.match(page.get("chart").innerHTML, /class="user-path"/);
  page.update({ budget: 1000 });
  assert.equal(page.get("budget-value").textContent, "10,000,000");
  page.update({ budget: -1 });
  assert.equal(page.get("error").hidden, false);
  assert.equal(page.get("figure").hidden, true);
  assert.equal(page.get("duration-figure").hidden, true);
  assert.equal(page.get("reward").textContent, "—");
  page.update({ budget: 500, duration: 0, start: 0 });
  assert(page.get("error").hidden);
  assert.equal(page.get("figure").hidden, false);
  assert.equal(page.get("duration-figure").hidden, false);
  assert.match(page.get("allocation").textContent, /Blocks 0–99/);
});

test("duration changes clamp the latest start and never discard part of the budget", () => {
  const page = app();
  page.update({ duration: 0 });
  page.update({ start: 9900 });
  assert.equal(page.get("duration-value").textContent, "100");
  assert.equal(page.get("start").max, "9900");
  assert.equal(page.get("start").value, "9900");
  assert.match(page.get("allocation").textContent, /Blocks 9,900–9,999/);
  page.update({ duration: 1000 });
  assert.equal(page.get("duration-value").textContent, "5,000");
  assert.equal(page.get("start").max, "5000");
  assert.equal(page.get("start").value, "5000");
  assert.equal(page.get("start-value").textContent, "5,000");
  assert.match(page.get("allocation").textContent, /Blocks 5,000–9,999/);
  page.update({ duration: 0 });
  assert.equal(page.get("start").max, "9900");
  assert.equal(page.get("start").value, "5000");
});

test("touch selection persists and keyboard inspection reaches both chart endpoints", () => {
  const page = app();
  const chart = page.get("chart");
  chart.listeners.pointerdown({ clientX: 500, pointerType: "touch" });
  const tapped = page.get("detail").textContent;
  assert.match(tapped, /^Block/);
  chart.listeners.pointerleave({ pointerType: "touch" });
  assert.equal(page.get("detail").textContent, tapped);
  chart.listeners.keydown({ key: "End", preventDefault() {} });
  assert.match(page.get("detail").textContent, /Block 9,999/);
  assert.equal(page.get("detail").attributes["aria-live"], "polite");
  chart.listeners.keydown({ key: "Home", preventDefault() {} });
  assert.match(page.get("detail").textContent, /Block 0 ·/);
  chart.listeners.pointerleave({ pointerType: "mouse" });
  assert.equal(page.get("inspection").hidden, true);
});

test("duration chart marks the exact selected reward and caches only compatible curves", () => {
  const page = app();
  const initial = page.getState().curve;
  page.update({ duration: 0 });
  assert.equal(page.getState().curve, initial);
  assert.match(page.get("duration-chart").innerHTML, /data-duration="100"/);
  assert.match(page.get("duration-chart").innerHTML, new RegExp(`data-reward="${page.getState().result.reward}"`));
  assert.match(page.get("duration-detail").textContent, /Selected window: 100 blocks/);
  for (const values of [{ budget: 500 }, { slope: -95 }, { trend: "sqrt" }, { start: 9000 }]) {
    const before = page.getState().curve;
    page.update(values);
    const { curve, rates, result } = page.getState();
    assert.notEqual(curve, before);
    close(curve[result.duration - 100].reward, simulate(rates, result.budget, result.duration, result.start).reward);
    assert.match(page.get("duration-chart").innerHTML, /class="duration-path"/);
    assert.doesNotMatch(page.get("duration-chart").innerHTML, /NaN|Infinity|undefined/);
  }
});

test("duration chart handles a single fitting duration and resizes without recomputing", () => {
  const page = app();
  page.update({ duration: 0 });
  page.update({ start: 9900 });
  const curve = page.getState().curve;
  assert.equal(curve.length, 1);
  assert.match(page.get("duration-chart").innerHTML, /id="selected-duration" cx="504"/);
  for (const width of [284, 600, 1200]) {
    page.get("duration-chart").width = width;
    page.redraw();
    assert.equal(page.get("duration-chart").attributes.viewBox, `0 0 ${width} 350`);
    assert.doesNotMatch(page.get("duration-chart").innerHTML, /NaN|Infinity|undefined/);
    assert.equal(page.getState().curve, curve);
  }
});

test("both charts share the noisy path and strategy changes do not reroll it", () => {
  const page = app();
  page.update({ noise: 30 });
  assert(page.get("error").hidden);
  assert.equal(page.get("noise-value").textContent, "30%");
  assert.equal(page.get("noise").attributes["aria-valuetext"], "30% standard deviation around the trend");
  assert.match(page.get("duration-scope").textContent, /Noise 30%/);
  const original = Array.from(page.getState().rates);
  const curve = page.getState().curve;
  page.update({ duration: 0 });
  assert.equal(page.getState().curve, curve);
  for (const values of [{ budget: 1000 }, { start: 9000 }]) {
    page.update(values);
    const { rates, result, curve } = page.getState();
    assert.deepEqual(Array.from(rates), original);
    close(curve[result.duration - 100].reward, simulate(rates, result.budget, result.duration, result.start).reward);
    assert.doesNotMatch(page.get("chart").innerHTML + page.get("duration-chart").innerHTML, /NaN|Infinity|undefined/);
  }
  const previous = page.getState().curve;
  page.update({ noise: 15 });
  assert.notEqual(page.getState().curve, previous);
  assert.notDeepEqual(Array.from(page.getState().rates), original);
  page.update({ noise: 30 });
  assert.deepEqual(Array.from(page.getState().rates), original);
  const reloaded = app();
  reloaded.update({ noise: 30 });
  assert.deepEqual(Array.from(reloaded.getState().rates), original);
});

test("noisy market and user lines use every block and cannot falsely cross", () => {
  const page = app();
  page.update({ noise: 30, budget: 0, duration: 1000, start: 4000 });
  const svg = page.get("chart").innerHTML;
  const points = name => svg.match(new RegExp(`class="${name}" d="([^"]+)"`))[1].split(" ").map(point => point.slice(1).split(",").map(Number));
  const market = points("market-path");
  const user = points("user-path");
  const { result } = page.getState();
  assert.equal(market.length, 10000);
  assert.equal(user.length, result.duration);
  user.forEach(([x, y], index) => {
    const [marketX, marketY] = market[result.start + index];
    assert.equal(x, marketX);
    assert(y <= marketY);
  });
});
