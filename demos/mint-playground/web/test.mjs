import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const source = await readFile(new URL('./app.js', import.meta.url), 'utf8');
const { BUDGET_MIN, BUDGET_MAX, DURATION_MIN, DURATION_MAX, DELAY_MAX, BLOCKS_PER_DAY, TIME_KEY, blockDate, burnAt, feeAt, projectedReward, PALETTE, userColor, contributionStatus, FEE, SPARKS, UNIT_KEY, formatMoney, parseBtcUsdQuote } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const plan = (options = {}) => ({ user_id: 1, budget_sats: 300005, duration: 100, start_block: 10, ...options });

test('budget is a native linear slider with the requested endpoints', async () => {
  const html = await readFile(new URL('./index.html', import.meta.url), 'utf8');
  const slider = html.match(/<input\b[^>]*\bid="budget"[^>]*>/)[0];
  const attribute = name => Number(slider.match(new RegExp(`\\b${name}="([^"]+)"`))[1]);
  assert.equal(BUDGET_MIN, 100000);
  assert.equal(BUDGET_MAX, 100000000);
  assert.equal(attribute('min'), BUDGET_MIN);
  assert.equal(attribute('max'), BUDGET_MAX);
  assert.equal(attribute('step'), 100000);
  assert.equal(attribute('value'), 1000000);
});

test('duration and delay sliders use exact integer block bounds at single-block resolution', async () => {
  const html = await readFile(new URL('./index.html', import.meta.url), 'utf8');
  assert.equal(BLOCKS_PER_DAY, 144);
  assert.equal(DURATION_MIN, 144);
  assert.equal(DURATION_MAX, 4464);
  assert.equal(DELAY_MAX, 4464);
  for (const [id, min, max, value] of [['duration', 144, 4464, 500], ['delay', 0, 4464, 0]]) {
    const slider = html.match(new RegExp(`<input\\b[^>]*\\bid="${id}"[^>]*>`))[0];
    const attribute = name => Number(slider.match(new RegExp(`\\b${name}="([^"]+)"`))[1]);
    assert.equal(attribute('min'), min);
    assert.equal(attribute('max'), max);
    assert.equal(attribute('step'), 1);
    assert.equal(attribute('value'), value);
  }
});

test('calendar dates start at block one and use exact 144-block UTC days, including leap and year boundaries', () => {
  assert.equal(blockDate(0), 'Jan 10');
  for (const block of [1, 2, 143, 144]) assert.equal(blockDate(block), 'Jan 11');
  assert.equal(blockDate(145), 'Jan 12');
  assert.equal(blockDate(61 * 144 + 1), 'Mar 13');
  assert.equal(blockDate(355 * 144), 'Dec 31');
  assert.equal(blockDate(355 * 144 + 1), 'Jan 1');
  assert.equal(blockDate(414 * 144 + 1), 'Feb 29');
  assert.equal(blockDate(415 * 144 + 1), 'Mar 1');
  assert.equal(blockDate(-1), '—');
});

test('integer schedule spreads the remainder into the first blocks and conserves sats', () => {
  const contribution = plan();
  assert.equal(burnAt(contribution, 9), 0);
  assert.equal(burnAt(contribution, 10), 2001);
  assert.equal(burnAt(contribution, 14), 2001);
  assert.equal(burnAt(contribution, 15), 2000);
  assert.equal(burnAt(contribution, 109), 2000);
  assert.equal(burnAt(contribution, 110), 0);
  let burn = 0;
  for (let block = 0; block < 200; block++) burn += burnAt(contribution, block);
  assert.equal(burn + contribution.duration * FEE, contribution.budget_sats);
});

test('mining fees count concurrent contributions, including the same address, not their budgets', () => {
  const contributions = [plan({ address: 'same-address', budget_sats: 100100 }), plan({ address: 'same-address', budget_sats: 100000000, start_block: 50 })];
  for (const [block, expected] of [[9, 0], [10, 1000], [49, 1000], [50, 2000], [109, 2000], [110, 1000], [149, 1000], [150, 0]]) {
    assert.equal(contributions.reduce((sum, contribution) => sum + feeAt(contribution, block), 0), expected);
  }
  assert.equal(contributions.reduce((sum, contribution) => {
    for (let block = 0; block < 200; block++) sum += feeAt(contribution, block);
    return sum;
  }, 0), 200 * FEE);
});

test('projection awards all issuance to a sole contribution and halves it with equal competition', () => {
  const draft = plan({ address: 'own', budget_sats: 300000, start_block: 11 });
  const competing = plan({ address: 'other', budget_sats: 300000, start_block: 11 });
  assert.equal(projectedReward([], draft, 'own'), 5000 * SPARKS);
  assert.equal(projectedReward([competing], draft, 'own'), 2500 * SPARKS);
});

test('projection uses each known competitor’s actual expiry rather than extrapolating its current rate', () => {
  const draft = plan({ address: 'own', budget_sats: 300000, start_block: 11 });
  const short = plan({ address: 'other', budget_sats: 150000, duration: 50, start_block: 11 });
  const long = plan({ address: 'other', budget_sats: 3000000, duration: 1000, start_block: 11 });
  assert.equal(burnAt(short, 11), burnAt(long, 11));
  assert.equal(projectedReward([short], draft, 'own'), 3750 * SPARKS);
  assert.equal(projectedReward([long], draft, 'own'), 2500 * SPARKS);
  assert.equal(burnAt(short, 60), 2000);
  assert.equal(burnAt(short, 61), 0, 'the last funded block is inclusive');
  assert.equal(burnAt(short, 999), 0);
});

test('future other-user schedules enter forecasts while expired contributions do not', () => {
  const draft = plan({ address: 'own', budget_sats: 300000, start_block: 11 });
  const expired = plan({ address: 'other', start_block: -90, budget_sats: 300000 });
  const scheduled = plan({ address: 'other', start_block: 61, duration: 50, budget_sats: 150000 });
  assert.equal(projectedReward([expired, scheduled], draft, 'own'), 3750 * SPARKS);
  assert.equal(burnAt(expired, 11), 0);
  assert.equal(burnAt(scheduled, 60), 0);
  assert.equal(burnAt(scheduled, 61), 2000);
  assert.equal(burnAt(scheduled, 110), 2000);
  assert.equal(burnAt(scheduled, 111), 0);
});

test('known own contributions reduce the draft’s allocated share and expire on schedule', () => {
  const draft = plan({ address: 'own', budget_sats: 300000, start_block: 11 });
  const existing = plan({ address: 'own', start_block: 11, budget_sats: 150000, duration: 50 });
  assert.equal(projectedReward([existing], draft, 'own'), 3750 * SPARKS);
  assert.equal(burnAt(existing, 60), 2000);
  assert.equal(burnAt(existing, 61), 0);
  const later = { ...existing, start_block: 61 };
  assert.equal(projectedReward([later], draft, 'own'), 3750 * SPARKS, 'known future overlap is included even if it has not begun');
});

test('projections preserve each actual remainder block and account for delayed windows beyond the chart', () => {
  const competing = plan({ address: 'other' });
  const draft = plan({ address: 'own', start_block: 15, duration: 1000, budget_sats: 3000000 });
  const estimate = projectedReward([competing], draft, 'own');
  assert.equal(burnAt(competing, 2000), 0, 'expired competition is not extrapolated');
  assert.equal(estimate, (95 * 25 + 905 * 50) * SPARKS, 'only the actual 95 overlapping blocks share rewards');
  assert.equal(burnAt(competing, 14), 2001, 'historical samples remain factual');
  const later = { ...draft, start_block: 5000 };
  assert.equal(projectedReward([competing], later, 'own'), 50000 * SPARKS, 'a delayed draft is evaluated over its full future duration');
});

test('a contribution finishes on the last inclusive block, not one block later', () => {
  assert.equal(contributionStatus(plan(), 9), 'scheduled');
  assert.equal(contributionStatus(plan(), 10), 'active');
  assert.equal(contributionStatus(plan(), 108), 'active');
  assert.equal(contributionStatus(plan(), 109), 'finished');
});

test('the fixed palette follows user ID modulo six with no shuffle or assignment state', () => {
  assert.deepEqual(PALETTE, ['#ffd800', '#ffc200', '#ff9c00', '#ff6922', '#ff4242', '#fff000']);
  assert.equal(userColor(1), '#ffc200', 'the first user has ID one, not zero');
  assert.equal(userColor(5), '#fff000');
  assert.equal(userColor(6), '#ffd800');
  assert.equal(userColor(7), '#ffc200');
  for (const id of [0, 1, 2, 3, 4, 5, 6, 7, 18, 999, Number.MAX_SAFE_INTEGER]) assert.equal(userColor(id), PALETTE[id % PALETTE.length]);
  assert.doesNotMatch(source, /Math\.random|shuffledPalette|addressColors|paletteKey/);
});

test('every referenced UI element exists once in the page', async () => {
  const html = await readFile(new URL('./index.html', import.meta.url), 'utf8');
  const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
  assert.equal(ids.length, new Set(ids).size, 'duplicate element IDs');
  for (const [, id] of source.matchAll(/element\('([^']+)'\)/g)) assert.ok(ids.includes(id), `missing #${id}`);
  assert.doesNotMatch(html, /\bcomposition(?:-label)?\b|type="checkbox"/, 'contribution colors have no toggle');
});

test('linear slider budgets conserve the complete budget at duration boundaries and representative single-block choices', () => {
  for (const budget_sats of [BUDGET_MIN, 200000, 500000, 1000000, 50000000, BUDGET_MAX]) {
    for (const duration of [DURATION_MIN, DURATION_MIN + 1, 500, 1440, DURATION_MAX - 1, DURATION_MAX]) {
      if (budget_sats < duration * (FEE + 1)) continue;
      const contribution = plan({ budget_sats, duration });
      let total = duration * FEE;
      for (let block = contribution.start_block; block < contribution.start_block + duration; block++) {
        const burn = burnAt(contribution, block);
        assert.ok(Number.isInteger(burn) && burn >= 1);
        total += burn;
      }
      assert.equal(total, budget_sats);
    }
  }
});

const quoteResponse = amount => ({ ok: true, json: async () => ({ data: { base: 'BTC', currency: 'USD', amount: String(amount) } }) });

async function openPage(pathname, storage = new Map(), post = async () => ({ id: 2, user_id: 1, start_block: 11 }), walletReady = Promise.resolve(), pageOptions = {}) {
  const html = await readFile(new URL('./index.html', import.meta.url), 'utf8');
  const draws = [];
  class Element {
    constructor(tag = 'div') { this.tag = tag; this.children = []; this.style = {}; this.handlers = {}; this.value = ''; this.hidden = false; this.disabled = false; }
    append(...children) { for (const child of children) this.children.push(...(child.tag === 'fragment' ? child.children : [child])); }
    replaceChildren(...children) { this.children = []; this.append(...children); }
    addEventListener(event, handler) { this.handlers[event] = handler; }
    setAttribute(name, value) { this[name] = value; }
    getContext() {
      return new Proxy({}, { get: (context, method) => (...args) => {
        draws.push({ method, args, fillStyle: context.fillStyle, strokeStyle: context.strokeStyle, globalAlpha: context.globalAlpha, canvas: this });
        if (method === 'createPattern') return { kind: 'pattern', canvas: args[0], repetition: args[1] };
        if (method === 'measureText') return { width: String(args[0]).length * 6 };
      } });
    }
    get clientWidth() { return 800; }
    get clientHeight() { return 275; }
  }
  const elements = new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([, id]) => [id, new Element()]));
  for (const [, id, text] of html.matchAll(/\bid="([^"]+)"[^>]*>([^<]*)</g)) elements.get(id).textContent = text.replaceAll('&nbsp;', '\u00a0');
  elements.get('budget').value = '1000000'; elements.get('duration').value = '500';
  elements.get('delay').value = '0';
  const requests = [], frames = [], timers = new Map(), abortTimeouts = [];
  let nextTimer = 0;
  const snapshot = { height: 10, totals: { contribution_count: 1, burned_sats: '100000', fees_sats: '1000', reward_sparks: '50000000000' }, contributions: [{ id: 1, user_id: 1, address: 'tf1demo-address', budget_sats: 300000, duration: 100, start_block: 10, burned_sats: '2000', fees_sats: '1000', reward_sparks: '5000000000' }] };
  const context = vm.createContext({
    document: { getElementById: id => elements.get(id), createElement: tag => new Element(tag), createDocumentFragment: () => new Element('fragment') },
    location: { pathname }, window: { devicePixelRatio: 1 }, console,
    AbortSignal: { timeout: delay => { abortTimeouts.push(delay); return AbortSignal.timeout(delay); } },
    localStorage: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value), removeItem: key => storage.delete(key) },
    crypto: { randomUUID: () => '11111111-1111-4111-8111-111111111111' },
    confirm: () => true,
    requestAnimationFrame: callback => frames.push(callback),
    ResizeObserver: class { observe() {} },
    setTimeout: (callback, delay) => { const id = ++nextTimer; timers.set(id, { callback, delay }); return id; },
    clearTimeout: id => timers.delete(id),
    walletModule: { default: async () => walletReady, generateMnemonic: () => 'a browser only recovery phrase', Wallet: { fromMnemonic: () => ({ nextAddress: () => ({ address: 'tf1demo-address' }), free() {} }) } },
    fetch: async (url, options = {}) => {
      requests.push({ url, options });
      if (url === '/api/state' || url === '/api/state?full_history=true') return { ok: true, json: async () => snapshot };
      if (url === 'https://api.coinbase.com/v2/prices/BTC-USD/spot') return pageOptions.quote ? pageOptions.quote() : quoteResponse(50000);
      assert.equal(url, '/api/contributions', 'unexpected request');
      return { ok: true, json: async () => post(JSON.parse(options.body)) };
    },
  });
  const script = source.replace(/^export /gm, '').replace("await import('/wallet/flamewallet_wasm.js')", 'await Promise.resolve(walletModule)');
  vm.runInContext(script, context);
  const settle = async () => { for (let i = 0; i < 4; i++) await new Promise(resolve => setImmediate(resolve)); while (frames.length) frames.shift()(); };
  await settle();
  return {
    elements, requests, storage, snapshot, draws, settle, timers, abortTimeouts,
    event: async (id, event) => { await elements.get(id).handlers[event]({ preventDefault() {} }); await settle(); },
    timer: async delay => {
      const pending = [...timers].find(([, timer]) => timer.delay === delay);
      assert.ok(pending, `no ${delay}ms timer scheduled`);
      const [id, timer] = pending;
      timers.delete(id);
      await timer.callback();
      await settle();
    },
    refresh: async () => {
      const [id, timer] = [...timers].find(([, timer]) => timer.delay === 1000);
      timers.delete(id);
      await timer.callback();
      await settle();
    },
  };
}

test('both charts center now between 500 past and 500 known future blocks', async () => {
  const html = await readFile(new URL('./index.html', import.meta.url), 'utf8');
  assert.match(html, /<canvas\b[^>]*aria-label="[^"]*Previous and next 500 blocks\./);
  for (const pathname of ['/', '/mint']) {
    const page = await openPage(pathname);
    const minting = pathname === '/mint';
    page.snapshot.height = minting ? 500 : 1500;
    page.snapshot.contributions = [plan({ id: 1, address: 'tf1demo-address', start_block: page.snapshot.height - 200, duration: 1000, budget_sats: 3000000 })];
    page.draws.length = 0;
    await page.refresh();
    const draws = page.draws.filter(call => call.canvas === page.elements.get('chart'));
    const ticks = draws.filter(call => call.method === 'fillText' && call.args[2] === 267);
    assert.deepEqual(ticks.map(call => call.args[0]), minting ? ['0', '250', '500', '750', '1,000'] : ['1,000', '1,250', '1,500', '1,750', '2,000']);
    assert.deepEqual(ticks.map(call => call.args[1]), [48, 224, 400, 576, 752]);
    const shading = draws.find(call => call.method === 'fillRect' && call.fillStyle === '#ffffff30');
    assert.equal(shading.args[0], 400, 'future shading begins at the centered current block');
    assert.equal(shading.args[2], 352, 'future shading ends at the right edge');
    const pathBefore = index => {
      const start = draws.slice(0, index).findLastIndex(call => call.method === 'beginPath');
      return draws.slice(start + 1, index).filter(call => call.method === 'moveTo' || call.method === 'lineTo');
    };
    const nowLine = pathBefore(draws.findLastIndex(call => call.method === 'stroke' && call.strokeStyle === '#666'));
    assert.deepEqual(nowLine.map(call => call.args), [[400, 14], [400, 248]], 'now marks the center of both charts');
    const band = pathBefore(draws.findIndex(call => call.method === 'fill' && call.fillStyle === userColor(1)));
    assert.equal(band.length, 2003, 'a closed band contains 1,001 top and 1,001 bottom samples plus its starting point');
    assert.equal(Math.min(...band.map(call => call.args[0])), 48);
    assert.equal(Math.max(...band.map(call => call.args[0])), 752);
    if (minting) {
      assert.match(page.elements.get('chart')['aria-label'], /Previous and next 500 blocks[, .]/);
      assert.match(page.elements.get('chart-note').textContent, /known|scheduled|future/i);
      const draft = pathBefore(draws.findIndex(call => call.method === 'stroke' && call.strokeStyle === userColor(1)));
      assert.equal(draft.length, 501, 'draft sampling stays within the visible future window');
      assert.equal(draft[0].args[0], 400);
      assert.equal(draft.at(-1).args[0], 752);
    }
  }
});

test('mint chart displays every known future schedule and expiry, with exact own overlays', async () => {
  const page = await openPage('/mint');
  const first = page.snapshot.contributions[0];
  page.snapshot.contributions = [
    { ...first, id: 1, user_id: 2, address: 'other', start_block: -89 },
    { ...first, id: 2, user_id: 3, address: 'other-future', start_block: 61, duration: 50, budget_sats: 150000 },
    { ...first, id: 3, user_id: 4, address: 'other-expired', start_block: -190 },
    { ...first, id: 4, start_block: 11, duration: 50, budget_sats: 150000 },
  ];
  page.draws.length = 0;
  await page.refresh();
  const paths = new Map();
  let path = [];
  for (const call of page.draws.filter(call => call.canvas === page.elements.get('chart'))) {
    if (call.method === 'beginPath') path = [];
    if (call.method === 'moveTo' || call.method === 'lineTo') path.push(call.args);
    if (call.method === 'fill') paths.set(call.fillStyle, path);
  }
  const thickness = (color, block) => {
    const index = block - (page.snapshot.height - 500);
    const path = paths.get(color);
    return path[2002 - index][1] - path[index + 1][1];
  };
  const otherHeight = thickness('#a3a3a3', 10);
  assert.ok(otherHeight > 0);
  for (const block of [11, 60, 111, 510]) assert.equal(thickness('#a3a3a3', block), 0, `no competing commitment is active at ${block}`);
  for (const block of [61, 110]) assert.equal(thickness('#a3a3a3', block), otherHeight, `the scheduled competitor is visible at ${block}`);
  assert.equal(thickness(userColor(1), 10), 0, 'own scheduled contribution has not started at now');
  assert.equal(thickness(userColor(1), 11), otherHeight, 'own concrete schedule begins next block');
  assert.equal(thickness(userColor(1), 60), otherHeight, 'own final funded block remains visible');
  assert.equal(thickness(userColor(1), 61), 0, 'own schedule expires rather than being extrapolated');
  assert.equal(thickness(userColor(1), 510), 0);
  assert.equal(page.elements.get('contributions').children.length, 1, 'only the wallet’s own schedule appears in its table');
  const draft = { budget_sats: 1000000, duration: 500, start_block: 11 };
  const estimate = projectedReward(page.snapshot.contributions, draft, first.address, 10);
  assert.equal(page.elements.get('projected-reward').textContent, new Intl.NumberFormat('en').format(Math.trunc(estimate / SPARKS)));
  assert.equal(page.elements.get('projected-rate').textContent, `${new Intl.NumberFormat('en').format(Math.trunc(draft.budget_sats * SPARKS / estimate))} sat / FLM`);
});

test('dashboard includes scheduled contributions in future bands and shows their budgets and date ranges', async () => {
  const page = await openPage('/');
  const first = page.snapshot.contributions[0];
  page.snapshot.height = 144;
  page.snapshot.contributions = [{ ...first, id: 2, user_id: 2, address: 'scheduled-user', start_block: 145, duration: 144, budget_sats: 432000, burned_sats: '0', fees_sats: '0', reward_sparks: '0' }];
  page.draws.length = 0;
  await page.refresh();
  const row = page.elements.get('contributions').children[0];
  assert.equal(row.children[1].children[0].textContent, 'Scheduled');
  assert.match(row.children[0].children[0].children[1].textContent, /432,000 sat \/ 144 blocks/);
  assert.match(row.children[0].children[1].textContent, /Blocks 145–288/);
  let path = [];
  let band;
  for (const call of page.draws.filter(call => call.canvas === page.elements.get('chart'))) {
    if (call.method === 'beginPath') path = [];
    if (call.method === 'moveTo' || call.method === 'lineTo') path.push(call.args);
    if (call.method === 'fill' && call.fillStyle === userColor(2)) band = path;
  }
  const thickness = block => {
    const index = block - (page.snapshot.height - 500);
    return band[2002 - index][1] - band[index + 1][1];
  };
  assert.equal(thickness(144), 0);
  assert.ok(thickness(145) > 0, 'the next-block scheduled start is visible');
  assert.ok(thickness(288) > 0, 'the final funded block is visible');
  assert.equal(thickness(289), 0, 'the known expiry ends the band');
  const totals = page.elements.get('contribution-totals').children[0].children.map(cell => cell.textContent);
  await page.event('time-days', 'click');
  const dayRow = page.elements.get('contributions').children[0];
  assert.match(dayRow.children[0].children[0].children[1].textContent, /432,000 sat \/ 1 day$/);
  assert.match(dayRow.children[0].children[1].textContent, /Jan 12–Jan 12/);
  assert.deepEqual(page.elements.get('contribution-totals').children[0].children.map(cell => cell.textContent), totals, 'calendar display does not alter all-time totals');
});

test('dashboard burn and patterned downward fees share one sats scale, with mint rates on the right', async () => {
  const page = await openPage('/');
  const pattern = page.draws.find(call => call.method === 'fill' && call.fillStyle?.kind === 'pattern').fillStyle;
  const strokes = page.draws.filter(call => call.canvas === pattern.canvas);
  page.snapshot.height = 109;
  page.draws.length = 0;
  await page.refresh();
  const areaPath = (fillStyle, outermost = false) => {
    const draws = page.draws.filter(call => call.canvas === page.elements.get('chart'));
    const fill = draws[outermost ? 'findLastIndex' : 'findIndex'](call => call.method === 'fill' && fillStyle(call.fillStyle));
    assert.ok(fill >= 0, 'area drawn');
    const start = draws.slice(0, fill).findLastIndex(call => call.method === 'beginPath');
    return draws.slice(start + 1, fill).filter(call => call.method === 'moveTo' || call.method === 'lineTo');
  };
  const feePath = () => areaPath(style => style?.kind === 'pattern');
  const one = feePath();
  const baseline = one[0].args[1];
  assert.ok(one.every(call => call.args[1] >= baseline), 'fee area never extends above zero');
  assert.equal(one.filter(call => call.args[1] > baseline).length, 100, 'exactly the scheduled blocks have fees');
  const oneDepth = Math.max(...one.map(call => call.args[1])) - baseline;
  assert.ok(oneDepth > 0);
  const burnPath = areaPath(style => style === userColor(1));
  const burnHeight = baseline - Math.min(...burnPath.map(call => call.args[1]));
  assert.ok(Math.abs(burnHeight - 2 * oneDepth) < 1e-10, '2,000 sats burned and 1,000 sats in fees use identical pixels per sat');
  assert.equal(pattern.repetition, 'repeat');
  const from = strokes.find(call => call.method === 'moveTo').args;
  const to = strokes.find(call => call.method === 'lineTo').args;
  assert.ok(from[0] !== to[0] && from[1] !== to[1], 'fee tile contains diagonal strokes');
  assert.ok(strokes.some(call => call.method === 'stroke' && call.strokeStyle === '#888'), 'fee strokes are grey');
  const ticks = page.draws.filter(call => call.method === 'fillText');
  const numericTick = text => Number(String(text).replaceAll(',', '').replace('−', '-').replace(/k$/, 'e3').replace(/m$/, 'e6'));
  let positive = false, negative = false;
  for (const left of ticks.filter(call => call.args[1] < 50 && Number.isFinite(numericTick(call.args[0])))) {
    const right = ticks.find(call => call.args[1] > 750 && call.args[2] === left.args[2] && Number.isFinite(numericTick(call.args[0])));
    assert.ok(right, 'both vertical scales label the same horizontal tick');
    const sats = numericTick(left.args[0]), rate = numericTick(right.args[0]);
    assert.ok(Math.abs(sats - 50 * rate) <= Math.max(1, Math.abs(sats) * .01), 'right axis is left axis divided by 50 FLM');
    positive ||= sats > 0;
    negative ||= sats < 0;
  }
  assert.ok(positive && negative, 'both burn and fee regions have paired axis labels');
  assert.ok(page.draws.some(call => call.method === 'fillRect' && call.fillStyle === '#ffffff30'), 'the dashboard distinguishes its visible future region');
  page.snapshot.contributions.push({ ...page.snapshot.contributions[0], id: 2, budget_sats: 100100 });
  page.draws.length = 0;
  await page.refresh();
  const two = feePath();
  const newBaseline = two[0].args[1];
  const twoDepth = Math.max(...two.map(call => call.args[1])) - newBaseline;
  const twoBurnHeight = newBaseline - Math.min(...areaPath(style => PALETTE.includes(style), true).map(call => call.args[1]));
  const participantFills = page.draws.filter(call => call.method === 'fill' && PALETTE.includes(call.fillStyle));
  assert.equal(participantFills.length, 1, 'contributions from the same user share one chart band');
  assert.ok(participantFills.every(call => call.fillStyle === userColor(1)));
  assert.ok(participantFills.every(call => call.globalAlpha === 1), 'participant colors are opaque, not washed out');
  assert.ok(Math.abs(twoDepth / twoBurnHeight - 2000 / 2001) < 1e-10, 'overlapping same-address contributions pay two fees on the shared scale');
  page.snapshot.contributions = [];
  page.draws.length = 0;
  await page.refresh();
  const emptyFees = feePath();
  assert.ok(emptyFees.every(call => call.args[1] === emptyFees[0].args[1]), 'empty blocks have no fee area');
  const mint = await openPage('/mint');
  assert.equal(mint.elements.get('fees-legend').hidden, true);
  assert.equal(mint.elements.get('chart-wrap').className, 'canvas-wrap');
  assert.equal(mint.elements.get('chart-title').textContent, 'Burn rate');
  assert.ok(!mint.draws.some(call => call.method === 'fill' && call.fillStyle?.kind === 'pattern'));
  for (const left of mint.draws.filter(call => call.method === 'fillText' && call.args[1] < 50 && numericTick(call.args[0]) > 0)) {
    const right = mint.draws.find(call => call.method === 'fillText' && call.args[1] > 750 && call.args[2] === left.args[2]);
    assert.ok(right, 'mint page also has a right-hand mint-rate scale');
    assert.equal(numericTick(left.args[0]), 50 * numericTick(right.args[0]));
  }
});

test('dashboard initializes, polls, draws, and renders the actual contribution snapshot', async () => {
  const page = await openPage('/');
  assert.equal(page.elements.get('total-burned').textContent, '100,000\u00a0sat');
  assert.equal(page.elements.get('total-reward').textContent, '500\u00a0FLM');
  assert.equal(page.elements.get('total-rate').textContent, '40\u00a0sat/FLM');
  assert.equal(page.elements.get('current-block').textContent, '10');
  assert.equal(page.elements.get('connection').textContent, 'Live');
  assert.equal(page.elements.get('contributions').children.length, 1);
  assert.match(page.elements.get('contributions').children[0].children[0].children[0].children[1].textContent, /#1.*300,000 sat.*100 blocks/, 'public rows disclose the known budget and duration');
  assert.equal(page.requests.filter(request => request.url === '/api/state').length, 1);
  assert.equal(page.requests.length, 2, 'state and BTC/USD quote are loaded independently');
  assert.equal(page.storage.size, 0, 'dashboard does not need palette or wallet storage');
});

test('dashboard requests recent contributions but uses server all-time totals, even with no visible rows', async () => {
  const page = await openPage('/');
  page.snapshot.totals = { contribution_count: 42, burned_sats: '9876543', fees_sats: '1234567', reward_sparks: '123456789999' };
  await page.refresh();
  const footer = () => page.elements.get('contribution-totals').children[0].children.map(cell => cell.textContent);
  assert.equal(page.elements.get('contributions').children.length, 1, 'only the returned recent row is displayed');
  assert.deepEqual(footer(), ['All-time total', '42 contributions', '9,876,543', '1,234', '1,234,567', '8,999']);
  assert.equal(page.elements.get('total-burned').textContent, '9,876,543\u00a0sat');
  assert.equal(page.elements.get('total-reward').textContent, '1,234\u00a0FLM');
  page.snapshot.contributions = [];
  page.draws.length = 0;
  await page.refresh();
  assert.equal(page.elements.get('contributions').children.length, 0);
  assert.equal(page.elements.get('empty-table').hidden, false);
  assert.equal(page.elements.get('empty-table').textContent, 'No recent contributions.');
  assert.ok(!page.draws.some(call => call.method === 'fill' && PALETTE.includes(call.fillStyle)), 'omitted contributions create no chart bands');
  assert.deepEqual(footer(), ['All-time total', '42 contributions', '9,876,543', '1,234', '1,234,567', '8,999']);
  assert.ok(page.requests.filter(request => request.url.startsWith('/api/state')).every(request => request.url === '/api/state'), 'dashboard never requests full history');
});

test('mint requests full history and retains personal totals for contributions finished more than 10,000 blocks ago', async () => {
  const page = await openPage('/mint');
  page.snapshot.height = 20000;
  page.snapshot.contributions[0].reward_sparks = '199999999';
  await page.refresh();
  assert.ok(page.requests.filter(request => request.url.startsWith('/api/state')).every(request => request.url === '/api/state?full_history=true'));
  assert.equal(page.elements.get('contributions').children.length, 1);
  assert.equal(page.elements.get('contributions').children[0].children[1].children[0].textContent, 'Finished');
  assert.equal(page.elements.get('total-burned').textContent, '2,000\u00a0sat');
  assert.equal(page.elements.get('total-reward').textContent, '1\u00a0FLM');
  assert.equal(page.elements.get('total-rate').textContent, '1,500\u00a0sat/FLM');
  assert.deepEqual(page.elements.get('contribution-totals').children[0].children.map(cell => cell.textContent), ['Total', '1 contributions', '2,000', '1', '1,000', '1,500']);
});

test('FLM and sat/FLM displays truncate fractions without changing rewards or projection calculations', async () => {
  for (const pathname of ['/', '/mint']) {
    const page = await openPage(pathname);
    const contribution = page.snapshot.contributions[0];
    contribution.burned_sats = '1000'; contribution.fees_sats = '999'; contribution.reward_sparks = '199999999';
    contribution.budget_sats = 300005;
    page.snapshot.totals = { contribution_count: 1, burned_sats: '1000', fees_sats: '999', reward_sparks: '199999999' };
    const before = JSON.stringify(page.snapshot);
    await page.refresh();
    const row = page.elements.get('contributions').children[0].children.map(cell => cell.textContent);
    const footer = page.elements.get('contribution-totals').children[0].children.map(cell => cell.textContent);
    assert.deepEqual(row.slice(2), ['1,000', '1', '999', '999']);
    assert.deepEqual(footer.slice(2), ['1,000', '1', '999', '999']);
    assert.equal(page.elements.get('total-reward').textContent, '1\u00a0FLM');
    assert.equal(page.elements.get('total-rate').textContent, `${pathname === '/mint' ? '999' : '40'}\u00a0sat/FLM`);
    if (pathname === '/mint') {
      const draft = { budget_sats: 1000000, duration: 500, start_block: page.snapshot.height + 1 };
      const estimate = projectedReward(page.snapshot.contributions, draft, 'tf1demo-address');
      assert.notEqual(estimate / SPARKS, Math.trunc(estimate / SPARKS));
      assert.equal(page.elements.get('projected-reward').textContent, new Intl.NumberFormat('en').format(Math.trunc(estimate / SPARKS)));
      assert.equal(page.elements.get('projected-rate').textContent, `${Math.trunc(draft.budget_sats * SPARKS / estimate)} sat / FLM`);
    }
    assert.equal(JSON.stringify(page.snapshot), before, 'display formatting never changes stored or received quantities');
    contribution.burned_sats = '7'; contribution.fees_sats = '0'; contribution.reward_sparks = '7000000';
    await page.refresh();
    assert.deepEqual(page.elements.get('contributions').children[0].children.slice(2).map(cell => cell.textContent), ['7', '0', '0', '100'], 'an exact integer rate does not lose one to floating-point division by FLM');
  }
});

test('mint-rate chart ticks truncate fractional rates, leaving the satoshi scale unchanged', async () => {
  const page = await openPage('/mint');
  page.snapshot.contributions[0].budget_sats = 100100;
  page.elements.get('budget').value = '100000';
  page.draws.length = 0;
  await page.refresh();
  const ticks = page.draws.filter(call => call.canvas === page.elements.get('chart') && call.method === 'fillText');
  const left = ticks.find(call => call.args[0] === '1' && call.args[1] < 50);
  assert.ok(left, 'the left axis still labels one satoshi');
  const right = ticks.find(call => call.args[1] > 750 && call.args[2] === left.args[2]);
  assert.equal(right.args[0], '0', '0.02 sat/FLM truncates only in the displayed axis label');
});

test('mint totals show only accrued wallet burns and rewards, with mining fees included in effective cost', async () => {
  const page = await openPage('/mint');
  const first = page.snapshot.contributions[0];
  page.snapshot.height = 250;
  page.snapshot.contributions = [
    { ...first, id: 1, start_block: 10, burned_sats: '110000', fees_sats: '100000', reward_sparks: '12345678901' },
    { ...first, id: 3, start_block: 200, burned_sats: '12345', fees_sats: '17000', reward_sparks: '987654321' },
    { ...first, id: 4, start_block: 300, budget_sats: 100000000, burned_sats: '0', fees_sats: '0', reward_sparks: '0' },
    { ...first, id: 5, user_id: 2, address: 'another-user', burned_sats: '999999', fees_sats: '999999', reward_sparks: '999999999999' },
  ];
  await page.refresh();
  const totals = () => ['total-burned', 'total-reward', 'total-rate'].map(id => page.elements.get(id).textContent);
  assert.deepEqual(totals(), ['122,345\u00a0sat', '133\u00a0FLM', '1,795\u00a0sat/FLM']);
  assert.equal(page.elements.get('total-rate-label').textContent, 'Effective rate');
  assert.match(page.elements.get('total-rate-label').title, /mining fees/);
  const table = page.elements.get('contribution-totals').children[0].children.map(cell => cell.textContent);
  assert.deepEqual(table.slice(2), ['122,345', '133', '117,000', '1,795'], 'header and table truncate only displayed reward and cost');
  assert.deepEqual(page.elements.get('contributions').children.map(row => row.children[1].children[0].textContent), ['Scheduled', 'Active', 'Finished']);
  page.elements.get('budget').value = String(BUDGET_MAX);
  await page.event('budget', 'input');
  page.elements.get('duration').value = '1000';
  await page.event('duration', 'input');
  assert.deepEqual(totals(), ['122,345\u00a0sat', '133\u00a0FLM', '1,795\u00a0sat/FLM'], 'draft budgets and projected rewards never change the accrued header');
});

test('mint effective cost is undefined before any FLM is earned, and a ready unregistered wallet has zero totals', async () => {
  const page = await openPage('/mint');
  page.snapshot.contributions[0].reward_sparks = '0';
  await page.refresh();
  assert.equal(page.elements.get('total-burned').textContent, '2,000\u00a0sat');
  assert.equal(page.elements.get('total-reward').textContent, '0\u00a0FLM');
  assert.equal(page.elements.get('total-rate').textContent, '—\u00a0sat/FLM');
  page.snapshot.contributions[0].address = 'another-user';
  await page.refresh();
  assert.equal(page.elements.get('total-burned').textContent, '0\u00a0sat');
  assert.equal(page.elements.get('total-reward').textContent, '0\u00a0FLM');
  assert.equal(page.elements.get('total-rate').textContent, '—\u00a0sat/FLM');
});

test('an unavailable wallet never substitutes global totals on the mint page', async () => {
  const storage = new Map([['flame-mint-playground-wallet-v1', '{broken json']]);
  const page = await openPage('/mint', storage);
  assert.deepEqual(['total-burned', 'total-reward', 'total-rate'].map(id => page.elements.get(id).textContent), ['—\u00a0sat', '—\u00a0FLM', '—\u00a0sat/FLM']);
  assert.equal(page.elements.get('projected-reward').textContent, '—', 'an unknown identity cannot distinguish its own future schedules');
  assert.equal(page.elements.get('projected-rate').textContent, '—');
  await page.refresh();
  assert.deepEqual(['total-burned', 'total-reward', 'total-rate'].map(id => page.elements.get(id).textContent), ['—\u00a0sat', '—\u00a0FLM', '—\u00a0sat/FLM']);
  assert.equal(page.elements.get('contributions').children.length, 0);
});

test('a wallet arriving after the first poll fills personal totals without waiting for another poll', async () => {
  let finishWallet;
  const walletReady = new Promise(resolve => { finishWallet = resolve; });
  const page = await openPage('/mint', new Map(), undefined, walletReady);
  assert.equal(page.elements.get('connection').textContent, 'Live', 'the snapshot arrived while wallet initialization is pending');
  assert.deepEqual(['total-burned', 'total-reward', 'total-rate'].map(id => page.elements.get(id).textContent), ['—\u00a0sat', '—\u00a0FLM', '—\u00a0sat/FLM']);
  finishWallet();
  await page.settle();
  assert.equal(page.elements.get('total-burned').textContent, '2,000\u00a0sat');
  assert.equal(page.elements.get('total-reward').textContent, '50\u00a0FLM');
  assert.equal(page.elements.get('total-rate').textContent, '60\u00a0sat/FLM');
  assert.equal(page.requests.filter(request => request.url === '/api/state?full_history=true').length, 1, 'wallet completion reuses the current full-history snapshot');
});

test('dashboard groups contributions by user ID, with the most recent contributor on top', async () => {
  const page = await openPage('/');
  const initialFills = page.draws.filter(call => call.method === 'fill' && PALETTE.includes(call.fillStyle));
  assert.deepEqual(initialFills.map(call => call.fillStyle), [userColor(1)]);
  assert.ok(initialFills.every(call => call.globalAlpha === 1), 'colors are opaque from the initial draw');
  const first = page.snapshot.contributions[0];
  page.snapshot.contributions.push(
    { ...first, id: 2, user_id: 26, address: 'next-user' },
    { ...first, id: 3 },
  );
  page.draws.length = 0;
  await page.refresh();
  const chartColors = page.draws.filter(call => call.method === 'fill' && PALETTE.includes(call.fillStyle)).map(call => call.fillStyle);
  assert.deepEqual(chartColors, [userColor(26), userColor(1)]);
  const tableColors = page.elements.get('contributions').children.map(row => row.children[0].children[0].children[0].style.background);
  assert.deepEqual(tableColors, [userColor(1), userColor(26), userColor(1)]);
});

test('dashboard sums each user’s burns and moves their complete band on top when they contribute again', async () => {
  const page = await openPage('/');
  page.snapshot.height = 12;
  const first = page.snapshot.contributions[0];
  const contributions = [
    { ...first, id: 1, budget_sats: 220000, start_block: 10 },
    { ...first, id: 2, user_id: 26, address: 'another-user', budget_sats: 430000, start_block: 8 },
    { ...first, id: 3, budget_sats: 760000, start_block: 9 },
  ];
  const bands = (page, colors = PALETTE, currentX = 400) => {
    let path = [];
    const result = [];
    for (const call of page.draws.filter(call => call.canvas === page.elements.get('chart'))) {
      if (call.method === 'beginPath') path = [];
      if (call.method === 'moveTo' || call.method === 'lineTo') path.push(call.args);
      if (call.method === 'fill' && colors.includes(call.fillStyle)) {
        const [top, base] = path.filter(point => point[0] === currentX).map(point => point[1]);
        assert.ok(Number.isFinite(top) && Number.isFinite(base));
        result.push({ color: call.fillStyle, top, base });
      }
    }
    return result;
  };
  const redraw = async (page, contributions) => {
    page.snapshot.contributions = contributions;
    page.draws.length = 0;
    await page.refresh();
  };
  const close = (actual, expected) => assert.ok(Math.abs(actual - expected) < 1e-10, `${actual} differs from ${expected}`);
  const reversed = contributions.slice().reverse();
  await redraw(page, reversed);
  assert.deepEqual(reversed.map(contribution => contribution.id), [3, 2, 1], 'drawing does not mutate the snapshot');
  const initial = bands(page);
  assert.deepEqual(initial.map(band => band.color), [userColor(26), userColor(1)]);
  const pixelsPerSat = (initial[0].base - initial[0].top) / burnAt(contributions[1], 12);
  let total = 0;
  for (const [index, burn] of [3300, 7800].entries()) {
    close(initial[index].base, initial[0].base - total * pixelsPerSat);
    total += burn;
    close(initial[index].top, initial[0].base - total * pixelsPerSat);
    assert.ok(initial[index].top < initial[index].base, 'each user has a positive band for their combined burns');
  }
  assert.deepEqual(page.elements.get('contributions').children.map(row => row.children[0].children[0].children[1].textContent.split(' · ')[0]), ['#3', '#2', '#1'], 'table remains newest first');
  await redraw(page, [contributions[1], contributions[0], contributions[2]]);
  assert.deepEqual(bands(page), initial, 'input order has no effect on band order or geometry');
  const latest = { ...first, id: 4, user_id: 26, address: 'another-user', budget_sats: 110000, start_block: 11 };
  await redraw(page, [latest, ...reversed]);
  const added = bands(page);
  assert.deepEqual(added.map(band => band.color), [userColor(1), userColor(26)], 'the returning user moves above the other user without changing color');
  close(added[0].base, initial[0].base);
  close(added[0].base - added[0].top, 7800 * pixelsPerSat);
  close(added[1].base, added[0].top);
  close(added[1].base - added[1].top, 3400 * pixelsPerSat);
  await redraw(page, [...contributions, latest]);
  assert.deepEqual(bands(page), added, 'the latest contribution ID determines user order, not snapshot order');
  await redraw(page, [{ ...latest, id: 5, user_id: 7, address: 'same-color-other-user' }, ...contributions, latest]);
  assert.deepEqual(bands(page).map(band => band.color), [userColor(1), userColor(26), userColor(7)], 'distinct users sharing a palette color remain separate bands');

  const mint = await openPage('/mint');
  mint.snapshot.height = 12;
  await redraw(mint, reversed);
  const ownAndOthers = bands(mint, ['#a3a3a3', userColor(1)], 400);
  assert.deepEqual(ownAndOthers.map(band => band.color), ['#a3a3a3', userColor(1)], 'mint page still groups others below all of the wallet’s contributions');
  close(ownAndOthers[1].base, ownAndOthers[0].top);
  close((ownAndOthers[1].base - ownAndOthers[1].top) / (ownAndOthers[0].base - ownAndOthers[0].top), 7800 / 3300);
});

test('mint chart, draft, legend, and table use the wallet user ID while other users stay grey', async () => {
  const page = await openPage('/mint');
  const first = page.snapshot.contributions[0];
  for (const user_id of [4, 26]) {
    page.snapshot.contributions = [
      { ...first, id: 3, user_id, address: first.address },
      { ...first, id: 2, user_id: 5, address: 'another-user' },
      { ...first, id: 1, user_id, address: first.address },
    ];
    page.draws.length = 0;
    await page.refresh();
    const color = userColor(user_id);
    const draws = page.draws.filter(call => call.canvas === page.elements.get('chart'));
    const fills = draws.filter(call => call.method === 'fill');
    assert.deepEqual(fills.map(call => call.fillStyle), ['#a3a3a3', color], 'other users remain grey regardless of their assigned palette color');
    assert.equal(fills[0].globalAlpha, .76, 'other users keep their existing grey appearance');
    assert.equal(fills[1].globalAlpha, 1, 'the wallet color is not faded');
    assert.ok(draws.some(call => call.method === 'stroke' && call.strokeStyle === color), 'draft line matches the assigned user color');
    assert.equal(page.elements.get('own-swatch').style.background, color);
    assert.equal(page.elements.get('draft-swatch').style.borderColor, color);
    const tableColors = page.elements.get('contributions').children.map(row => row.children[0].children[0].children[0].style.background);
    assert.deepEqual(tableColors, [color, color], 'accepted contributions and the chart share the same color');
  }
});

test('mint draft stays grey until a poll provides the wallet address’s assigned user ID', async () => {
  const page = await openPage('/mint');
  const first = page.snapshot.contributions[0];
  page.snapshot.contributions = [{ ...first, id: 5, user_id: 5, address: 'another-user' }];
  page.draws.length = 0;
  await page.refresh();
  assert.equal(page.elements.get('own-swatch').style.background, '#888');
  assert.equal(page.elements.get('draft-swatch').style.borderColor, '#888');
  assert.ok(page.draws.some(call => call.method === 'stroke' && call.strokeStyle === '#888'), 'an unregistered wallet does not guess its eventual user ID');
  assert.ok(page.draws.some(call => call.method === 'fill' && call.fillStyle === '#a3a3a3'));
  page.snapshot.contributions.push({ ...first, id: 6, user_id: 26 });
  page.draws.length = 0;
  await page.refresh();
  const color = userColor(26);
  assert.equal(page.elements.get('own-swatch').style.background, color);
  assert.equal(page.elements.get('draft-swatch').style.borderColor, color);
  assert.ok(page.draws.some(call => call.method === 'fill' && call.fillStyle === color));
  assert.ok(page.draws.some(call => call.method === 'stroke' && call.strokeStyle === color), 'the next snapshot switches to the server-assigned color');
});

test('user colors match across clients, reloads, reordered snapshots, and repeated contributions', async () => {
  const page = await openPage('/');
  const colorById = page => new Map(page.elements.get('contributions').children.map(row => [row.children[0].children[0].children[1].textContent.split(' · ')[0], row.children[0].children[0].children[0].style.background]));
  const first = page.snapshot.contributions[0];
  page.snapshot.contributions = Array.from({ length: 8 }, (_, index) => ({ ...first, id: 9 - index, user_id: index === 0 ? first.user_id : 9 - index, address: index === 0 ? first.address : `user-${9 - index}` })).concat(first);
  await page.refresh();
  const colors = colorById(page);
  assert.equal(colors.get('#1'), PALETTE[1]);
  assert.equal(colors.get('#2'), PALETTE[2]);
  assert.equal(colors.get('#6'), PALETTE[0]);
  assert.equal(colors.get('#7'), PALETTE[1], 'user ID seven wraps around to user ID one');
  assert.equal(colors.get('#8'), PALETTE[2]);
  assert.equal(colors.get('#9'), PALETTE[1], 'repeat user keeps their color regardless of contribution ID');
  page.snapshot.contributions.unshift({ ...first, id: 10, user_id: 26, address: 'next-user' });
  await page.refresh();
  assert.equal(colorById(page).get('#10'), PALETTE[2]);
  for (const [id, color] of colors) assert.equal(colorById(page).get(id), color, 'polls and additions do not recolor users');
  for (const storage of [page.storage, new Map(), new Map([['flame-mint-playground-palette-v1', JSON.stringify(PALETTE.slice().reverse())]])]) {
    const before = new Map(storage);
    const other = await openPage('/', storage);
    other.snapshot.contributions = page.snapshot.contributions.slice().reverse();
    await other.refresh();
    assert.deepEqual(colorById(other), colorById(page));
    assert.deepEqual(storage, before, 'palette storage is neither read nor rewritten');
  }
  assert.equal(page.storage.size, 0);
});

test('dashboard colors need no storage beyond independent unit and time preferences', async () => {
  const page = await openPage('/', { get(key) { assert.ok([UNIT_KEY, TIME_KEY].includes(key)); return null; }, set() { assert.fail('Unexpected storage write'); } });
  assert.equal(page.elements.get('connection').textContent, 'Live');
  const firstColor = page.elements.get('contributions').children[0].children[0].children[0].children[0].style.background;
  assert.equal(firstColor, userColor(1));
  await page.refresh();
  assert.equal(page.elements.get('contributions').children[0].children[0].children[0].children[0].style.background, firstColor);
});

test('the top mint rate follows the current block, ignoring historical totals and future plans', async () => {
  const page = await openPage('/');
  page.snapshot.contributions.push({ ...page.snapshot.contributions[0], id: 2, budget_sats: 200000, start_block: 11 });
  await page.refresh();
  assert.equal(page.elements.get('total-rate').textContent, '40\u00a0sat/FLM', 'future contribution does not affect this block');
  page.snapshot.height = 11;
  await page.refresh();
  assert.equal(page.elements.get('total-rate').textContent, '60\u00a0sat/FLM', '3,000 sats burned this block / 50 FLM');
  page.snapshot.height = 110;
  await page.refresh();
  assert.equal(page.elements.get('total-rate').textContent, '20\u00a0sat/FLM', 'first contribution no longer burns');
  page.snapshot.height = 111;
  await page.refresh();
  assert.equal(page.elements.get('total-rate').textContent, '—\u00a0sat/FLM', 'no issuance in an empty block');
});

test('budget slider values are satoshis directly, including its linear midpoint', async () => {
  const page = await openPage('/mint');
  for (const value of [BUDGET_MIN, 50000000, 50100000, BUDGET_MAX]) {
    page.elements.get('budget').value = String(value);
    await page.event('budget', 'input');
    assert.equal(page.elements.get('budget-value').textContent, `${new Intl.NumberFormat('en').format(value)} sat`);
    assert.equal(page.elements.get('submit').disabled, value === BUDGET_MIN, 'unfunded fee combinations cannot be submitted');
  }
});

test('blocks and days are accessible persisted display modes independent from currency and raw contribution data', async () => {
  for (const pathname of ['/', '/mint']) {
    const page = await openPage(pathname);
    const before = JSON.stringify(page.snapshot);
    assert.equal(page.elements.get('time-blocks')['aria-pressed'], 'true');
    assert.equal(page.elements.get('time-days')['aria-pressed'], 'false');
    assert.equal(page.storage.has(TIME_KEY), false);
    page.snapshot.height = 144 * 61 + 1;
    await page.refresh();
    page.draws.length = 0;
    await page.event('time-days', 'click');
    assert.equal(page.elements.get('time-days')['aria-pressed'], 'true');
    assert.equal(page.elements.get('time-blocks')['aria-pressed'], 'false');
    assert.equal(page.storage.get(TIME_KEY), 'days');
    assert.equal(page.elements.get('current-block').textContent, 'Mar 13');
    const ticks = page.draws.filter(call => call.canvas === page.elements.get('chart') && call.method === 'fillText' && call.args[2] === 267);
    assert.equal(ticks.length, 5);
    assert.equal(ticks[2].args[0], 'Mar 13', 'the center date represents the current block');
    assert.ok(ticks.every(call => /^[A-Z][a-z]{2} \d{1,2}$/.test(call.args[0])), 'all tick labels use short calendar dates');
    const dates = ticks.map(call => call.args[0]);
    await page.event('unit-btc', 'click');
    assert.equal(page.storage.get(TIME_KEY), 'days');
    assert.equal(page.storage.get(UNIT_KEY), 'BTC');
    assert.equal(page.elements.get('time-days')['aria-pressed'], 'true');
    const reloaded = await openPage(pathname, page.storage);
    assert.equal(reloaded.elements.get('time-days')['aria-pressed'], 'true');
    assert.equal(reloaded.elements.get('unit-btc')['aria-pressed'], 'true');
    assert.deepEqual(page.snapshot.contributions, JSON.parse(before).contributions, 'display changes never alter committed schedules');
    assert.equal(new Set(dates).size, 5);
  }
});

test('invalid or unavailable time preference storage falls back to blocks without breaking either switch', async () => {
  const storage = new Map([[TIME_KEY, 'invalid']]);
  const invalid = await openPage('/', storage);
  assert.equal(invalid.elements.get('time-blocks')['aria-pressed'], 'true');
  assert.equal(storage.get(TIME_KEY), 'invalid');
  const page = await openPage('/', { get() { throw new Error('Storage blocked'); }, set() { throw new Error('Storage blocked'); } });
  await page.event('time-days', 'click');
  assert.equal(page.elements.get('time-days')['aria-pressed'], 'true');
  assert.equal(page.elements.get('connection').textContent, 'Live');
  await page.event('unit-usd', 'click');
  assert.equal(page.elements.get('time-days')['aria-pressed'], 'true');
  assert.equal(page.elements.get('total-burned').textContent, '$50.000');
});

test('day-mode duration and delay show block-derived days while retaining every single-block slider value', async () => {
  const page = await openPage('/mint');
  await page.event('time-days', 'click');
  assert.match(page.elements.get('duration-min').textContent, /^1 day/);
  assert.match(page.elements.get('duration-max').textContent, /^31 day/);
  assert.match(page.elements.get('delay-min').textContent, /^0 day/);
  assert.match(page.elements.get('delay-max').textContent, /^31 day/);
  page.elements.get('duration').value = '145';
  await page.event('duration', 'input');
  page.elements.get('delay').value = '1';
  await page.event('delay', 'input');
  assert.equal(page.elements.get('duration-value').textContent, '1.007 days');
  assert.equal(page.elements.get('delay-value').textContent, '0.007 days');
  assert.equal(page.elements.get('duration').value, '145');
  assert.equal(page.elements.get('delay').value, '1');
  await page.event('contribution-form', 'submit');
  const request = JSON.parse(page.requests.find(request => request.url === '/api/contributions').options.body);
  assert.equal(request.duration, 145);
  assert.equal(request.delay_blocks, 1);
  assert.equal(request.budget_sats, 1000000);
});

test('scheduled delayed drafts compete only across their actual start and expiry and survive retries exactly', async () => {
  const storage = new Map();
  const page = await openPage('/mint', storage, async () => { throw new Error('Response lost'); });
  page.elements.get('duration').value = '144';
  page.elements.get('delay').value = '4464';
  await page.event('duration', 'input');
  await page.event('delay', 'input');
  page.snapshot.contributions = [plan({ id: 1, address: 'other', start_block: 4475, duration: 144, budget_sats: 1000000 })];
  await page.refresh();
  assert.equal(page.elements.get('projected-reward').textContent, '3,600', 'all 144 delayed blocks overlap equal known competition');
  await page.event('contribution-form', 'submit');
  const request = JSON.parse(page.requests.find(request => request.url === '/api/contributions').options.body);
  assert.equal(request.delay_blocks, 4464);
  assert.equal(request.duration, 144);
  assert.equal(page.elements.get('delay').disabled, true);
  await page.event('time-days', 'click');
  await page.event('unit-usd', 'click');
  assert.deepEqual(JSON.parse(storage.get('flame-mint-playground-pending-v1')), request);
  const retry = await openPage('/mint', storage);
  assert.equal(Number(retry.elements.get('delay').value), 4464);
  assert.equal(Number(retry.elements.get('duration').value), 144);
  assert.equal(retry.elements.get('delay').disabled, true);
  await retry.event('contribution-form', 'submit');
  assert.deepEqual(JSON.parse(retry.requests.find(request => request.url === '/api/contributions').options.body), request);
});

test('the dashed draft is limited to its real scheduled interval and disappears outside the chart', async () => {
  const page = await openPage('/mint');
  page.snapshot.height = 500;
  page.elements.get('duration').value = '144';
  await page.refresh();
  const strokePath = () => {
    let path = [];
    for (const call of page.draws.filter(call => call.canvas === page.elements.get('chart'))) {
      if (call.method === 'beginPath') path = [];
      if (call.method === 'moveTo' || call.method === 'lineTo') path.push(call.args);
      if (call.method === 'stroke' && call.strokeStyle === userColor(1)) return path;
    }
    return [];
  };
  for (const [delay, firstBlock, lastBlock] of [[0, 500, 645], [100, 600, 745], [499, 999, 1000]]) {
    page.draws.length = 0;
    page.elements.get('delay').value = String(delay);
    await page.event('delay', 'input');
    const path = strokePath();
    assert.ok(path.length > 0);
    const xs = path.map(point => point[0]);
    assert.equal(Math.min(...xs), 48 + firstBlock / 1000 * 704, 'only one zero sample immediately precedes the actual start');
    assert.equal(Math.max(...xs), 48 + lastBlock / 1000 * 704, 'only one zero sample immediately follows the actual end, clipped to the window');
    assert.equal(path.length, lastBlock - firstBlock + 1);
  }
  for (const delay of [500, 4464]) {
    page.draws.length = 0;
    page.elements.get('delay').value = String(delay);
    await page.event('delay', 'input');
    assert.deepEqual(strokePath(), [], `delay ${delay} starts after the visible future window`);
    assert.notEqual(page.elements.get('projected-reward').textContent, '—', 'offscreen drafts still have full-duration estimates');
  }
});

test('legacy saved requests normalize absent delay to zero without rewriting their identity or historical duration', async () => {
  const pending = { address: 'tf1demo-address', request_id: 'legacy-request-001', budget_sats: 2000123, duration: 100 };
  const storage = new Map([['flame-mint-playground-pending-v1', JSON.stringify(pending)]]);
  const page = await openPage('/mint', storage);
  assert.equal(page.elements.get('submit').textContent, 'Retry pending');
  assert.match(page.elements.get('duration-value').textContent, /100 blocks/);
  assert.equal(page.elements.get('delay').disabled, true);
  page.elements.get('duration').value = '144';
  page.elements.get('delay').value = '99';
  await page.event('contribution-form', 'submit');
  assert.deepEqual(JSON.parse(page.requests.find(request => request.url === '/api/contributions').options.body), { ...pending, delay_blocks: 0 });
});

test('invalid saved delay or duration blocks new submissions without losing the saved request', async () => {
  const valid = { address: 'tf1demo-address', request_id: 'saved-request-0001', budget_sats: 10000000, duration: 144, delay_blocks: 0 };
  for (const change of [{ delay_blocks: -1 }, { delay_blocks: 4465 }, { delay_blocks: 0.5 }, { delay_blocks: '1' }, { duration: 143 }, { duration: 4465 }, { duration: 144.5 }]) {
    const saved = JSON.stringify({ ...valid, ...change });
    const storage = new Map([['flame-mint-playground-pending-v1', saved]]);
    const page = await openPage('/mint', storage);
    assert.equal(page.elements.get('submit').disabled, true);
    assert.equal(page.elements.get('delay').disabled, true);
    assert.match(page.elements.get('submit-message').textContent, /unreadable/);
    assert.equal(storage.get('flame-mint-playground-pending-v1'), saved);
  }
});

test('submitting locks budget, duration and delay until acceptance while display switches remain independent', async () => {
  let accept;
  const page = await openPage('/mint', new Map(), () => new Promise(resolve => { accept = resolve; }));
  page.elements.get('delay').value = '17';
  await page.event('delay', 'input');
  const submitting = page.event('contribution-form', 'submit');
  await page.settle();
  for (const id of ['budget', 'duration', 'delay', 'submit', 'discard-pending']) assert.equal(page.elements.get(id).disabled, true, id);
  await page.event('time-days', 'click');
  await page.event('unit-btc', 'click');
  const request = JSON.parse(page.requests.find(request => request.url === '/api/contributions').options.body);
  assert.equal(request.delay_blocks, 17);
  assert.equal(request.duration, 500);
  accept({ id: 2, user_id: 1, start_block: 28 });
  await submitting;
  for (const id of ['budget', 'duration', 'delay', 'submit']) assert.equal(page.elements.get(id).disabled, false, id);
  assert.match(page.elements.get('submit-message').textContent, /accepted/);
});

test('mint page keeps keys local and persists the exact retry request across a page reload', async () => {
  const storage = new Map();
  const first = await openPage('/mint', storage, async () => { throw new Error('Response lost'); });
  assert.equal(first.elements.get('page-title').textContent, 'Mint');
  assert.equal(first.elements.get('navigation').textContent, 'Dashboard');
  assert.equal(first.elements.get('empty-table').textContent, 'No contributions.');
  assert.equal(first.elements.get('wallet-address').textContent, 'tf1demo-address');
  assert.equal(first.elements.get('wallet-status').textContent, 'Saved locally. Keys stay in this browser.');
  assert.equal(first.elements.get('submit').disabled, false);
  assert.equal(first.elements.get('submit').textContent, 'Submit');
  assert.equal(first.elements.get('seed-phrase').textContent, '');
  await first.event('contribution-form', 'submit');
  const original = JSON.parse(first.requests.find(request => request.url === '/api/contributions').options.body);
  assert.deepEqual(Object.keys(original).sort(), ['address', 'budget_sats', 'delay_blocks', 'duration', 'request_id']);
  assert.equal(original.delay_blocks, 0);
  assert.equal(first.elements.get('budget').disabled, true);
  assert.equal(first.elements.get('duration').disabled, true);
  assert.equal(first.elements.get('delay').disabled, true);
  assert.ok(storage.has('flame-mint-playground-pending-v1'));
  const reloaded = await openPage('/mint', storage);
  assert.equal(reloaded.elements.get('budget').disabled, true);
  assert.equal(reloaded.elements.get('submit').textContent, 'Retry pending');
  await reloaded.event('contribution-form', 'submit');
  assert.deepEqual(JSON.parse(reloaded.requests.find(request => request.url === '/api/contributions').options.body), original);
  assert.equal(storage.has('flame-mint-playground-pending-v1'), false);
  assert.equal(reloaded.elements.get('budget').disabled, false);
  assert.equal(reloaded.elements.get('delay').disabled, false);
  assert.match(reloaded.elements.get('submit-message').textContent, /accepted/);
});

test('pending requests preserve exact non-step budgets after the slider migration', async () => {
  const pending = { address: 'tf1demo-address', request_id: 'old-request-00000001', budget_sats: 2000123, duration: 500 };
  const storage = new Map([['flame-mint-playground-pending-v1', JSON.stringify(pending)]]);
  const page = await openPage('/mint', storage);
  assert.equal(page.elements.get('budget').disabled, true);
  assert.equal(page.elements.get('budget-value').textContent, '2,000,123 sat');
  assert.equal(page.elements.get('submit').textContent, 'Retry pending');
  // Native range inputs can normalize a non-step value; the saved request stays authoritative.
  page.elements.get('budget').value = '2000000';
  await page.event('contribution-form', 'submit');
  assert.deepEqual(JSON.parse(page.requests.find(request => request.url === '/api/contributions').options.body), { ...pending, delay_blocks: 0 });
});

test('a corrupted saved wallet reports a recoverable error instead of silently replacing the identity', async () => {
  const storage = new Map([['flame-mint-playground-wallet-v1', '{broken json']]);
  const page = await openPage('/mint', storage);
  assert.match(page.elements.get('wallet-status').textContent, /Wallet unavailable/);
  assert.equal(page.elements.get('submit').disabled, true);
  assert.equal(page.elements.get('reset-wallet').hidden, false);
  assert.equal(storage.get('flame-mint-playground-wallet-v1'), '{broken json');
});

test('a saved reward address must match the address derived from its saved recovery phrase', async () => {
  const saved = JSON.stringify({ mnemonic: 'a browser only recovery phrase', address: 'tf1different-address' });
  const storage = new Map([['flame-mint-playground-wallet-v1', saved]]);
  const page = await openPage('/mint', storage);
  assert.match(page.elements.get('wallet-status').textContent, /does not match/);
  assert.equal(page.elements.get('submit').disabled, true);
  assert.equal(storage.get('flame-mint-playground-wallet-v1'), saved);
});

test('money formatting preserves eight BTC decimals, truncates display values, and keeps small USD rates', () => {
  assert.equal(formatMoney(123456789), '123,456,789');
  assert.equal(formatMoney(123456789, 'BTC'), '1.23456789');
  assert.equal(formatMoney(1, 'BTC'), '0.00000001');
  assert.equal(formatMoney(0, 'BTC'), '0.00000000');
  assert.equal(formatMoney(100000000, 'BTC'), '1.00000000');
  assert.equal(formatMoney(1.99, 'BTC', null, true), '0.00000001');
  assert.equal(formatMoney(123456789, 'USD', 50000), '61,728.39');
  assert.equal(formatMoney(1999, 'USD', 50000), '0.99');
  assert.equal(formatMoney(1.99, 'USD', 50000, true), '0.000995', 'conversion uses the raw rate, not its truncated sat display');
  assert.equal(formatMoney(2000, 'USD', 50000, true), '1.00');
  assert.equal(formatMoney(1000, 'USD'), '—');
  assert.equal(formatMoney(1000, 'USD', NaN), '—');
  assert.equal(formatMoney(1000, 'USD', 0), '—');
});

test('the quote parser accepts only positive finite USD prices of BTC', () => {
  assert.equal(parseBtcUsdQuote({ data: { base: 'BTC', currency: 'USD', amount: '50000.12' } }), 50000.12);
  for (const amount of ['', ' ', '0', '-1', 'NaN', 'Infinity', '1e999', 50000, null]) {
    assert.throws(() => parseBtcUsdQuote({ data: { base: 'BTC', currency: 'USD', amount } }));
  }
  for (const payload of [null, {}, { data: {} }, { data: { base: 'ETH', currency: 'USD', amount: '50000' } }, { data: { base: 'BTC', currency: 'EUR', amount: '50000' } }]) {
    assert.throws(() => parseBtcUsdQuote(payload));
  }
});

const quoteRequests = page => page.requests.filter(request => request.url.startsWith('https:'));
const stateRequests = page => page.requests.filter(request => request.url.startsWith('/api/state'));
const rowValues = page => page.elements.get('contributions').children[0].children.slice(2).map(cell => cell.textContent);
const footerValues = page => page.elements.get('contribution-totals').children[0].children.slice(2).map(cell => cell.textContent);
const chartGeometry = page => page.draws.filter(call => call.canvas === page.elements.get('chart') && ['moveTo', 'lineTo'].includes(call.method)).map(call => [call.method, call.args[1]]);

test('both pages offer an accessible three-state unit selection while always showing the BTC/USD quote', async () => {
  const html = await readFile(new URL('./index.html', import.meta.url), 'utf8');
  const quoteRow = html.match(/<p\b[^>]*id="price-status"[^>]*>[^<]*<\/p>/)[0];
  assert.doesNotMatch(quoteRow, /\bhidden\b/);
  assert.match(quoteRow, />Loading BTC\/USD…<\/p>/);
  assert.match(html, /<div><p>Earned<\/p><p[^>]*id="total-reward">—&nbsp;FLM<\/p><\/div>/);
  for (const pathname of ['/', '/mint']) {
    const page = await openPage(pathname);
    for (const [unit, selected] of [['sat', true], ['btc', false], ['usd', false]]) {
      assert.equal(page.elements.get(`unit-${unit}`)['aria-pressed'], String(selected));
    }
    assert.equal(quoteRequests(page).length, 1, 'the reference quote is fetched at startup in sat mode');
    assert.equal(page.elements.get('price-status').hidden, false);
    assert.match(page.elements.get('price-status').textContent, /BTC\/USD.*50,000|50,000.*BTC\/USD/);
    assert.equal(page.storage.has(UNIT_KEY), false, 'the default does not need a storage write');
    await page.event('unit-btc', 'click');
    assert.equal(page.elements.get('unit-btc')['aria-pressed'], 'true');
    assert.equal(page.elements.get('unit-sat')['aria-pressed'], 'false');
    assert.equal(quoteRequests(page).length, 1, 'unit changes reuse the loaded quote');
    assert.equal(stateRequests(page).length, 1, 'changing the unit reuses the same snapshot');
    assert.equal(page.elements.get('price-status').hidden, false);
    assert.equal(page.storage.get(UNIT_KEY), 'BTC');
  }
});

test('unit switches reuse the preloaded BTC/USD quote and preserve its refresh timer on both pages', async () => {
  for (const pathname of ['/', '/mint']) {
    const page = await openPage(pathname);
    const row = page.elements.get('price-status').textContent;
    const quoteTimer = [...page.timers].find(([, timer]) => timer.delay === 60000)[0];
    for (const [unit, amount, rate] of [
      ['usd', pathname === '/' ? '$50.000' : '$1.000', pathname === '/' ? '0.02\u00a0$/FLM' : '0.03\u00a0$/FLM'],
      ['btc', pathname === '/' ? '0.00100000\u00a0btc' : '0.00002000\u00a0btc', pathname === '/' ? '0.00000040\u00a0BTC/FLM' : '0.00000060\u00a0BTC/FLM'],
      ['sat', pathname === '/' ? '100,000\u00a0sat' : '2,000\u00a0sat', pathname === '/' ? '40\u00a0sat/FLM' : '60\u00a0sat/FLM'],
    ]) {
      await page.event(`unit-${unit}`, 'click');
      assert.equal(page.elements.get('total-burned').textContent, amount);
      assert.equal(page.elements.get('total-reward').textContent, pathname === '/' ? '500\u00a0FLM' : '50\u00a0FLM');
      assert.equal(page.elements.get('total-rate').textContent, rate);
      assert.equal(page.elements.get('total-burned-label').textContent, 'Burned');
      assert.equal(page.elements.get('total-rate-label').textContent, pathname === '/' ? 'Current rate' : 'Effective rate');
      assert.equal(page.elements.get('price-status').hidden, false);
      assert.equal(page.elements.get('price-status').textContent, row, 'reference quote does not depend on the display unit');
      assert.equal(quoteRequests(page).length, 1, 'switching units never fetches a price');
      assert.equal([...page.timers].find(([, timer]) => timer.delay === 60000)[0], quoteTimer, 'switching units does not reset the refresh timer');
    }
  }
});

test('a pending quote refresh retains its previous value and USD display until success or failure', async () => {
  let calls = 0, finish;
  const page = await openPage('/', new Map(), undefined, undefined, { quote: () => ++calls === 1 ? quoteResponse(50000) : new Promise(resolve => { finish = resolve; }) });
  await page.event('unit-usd', 'click');
  const row = page.elements.get('price-status').textContent;
  const pending = page.timer(60000);
  await page.settle();
  assert.equal(page.elements.get('price-status').textContent, row, 'a refresh does not blink to Loading');
  assert.equal(page.elements.get('total-burned').textContent, '$50.000');
  await page.event('unit-sat', 'click');
  await page.event('unit-usd', 'click');
  assert.equal(quoteRequests(page).length, 2, 'unit changes do not duplicate the pending refresh');
  finish({ ok: false, status: 503 });
  await pending;
  assert.equal(page.elements.get('total-burned').textContent, '$—', 'failure clears the old quote');
  assert.equal(page.elements.get('price-status').hidden, false);
  assert.match(page.elements.get('price-status').textContent, /unavailable/i);
  assert.equal([...page.timers.values()].filter(timer => timer.delay === 60000).length, 1, 'failed refresh schedules exactly one retry');
});

test('BTC mode converts totals, tables, and rates on both pages without touching FLM rewards', async () => {
  for (const pathname of ['/', '/mint']) {
    const page = await openPage(pathname);
    const snapshotBefore = JSON.stringify(page.snapshot);
    const rewardBefore = page.elements.get('total-reward').textContent;
    await page.event('unit-btc', 'click');
    assert.equal(page.elements.get('total-burned').textContent, `${pathname === '/' ? '0.00100000' : '0.00002000'}\u00a0btc`);
    assert.equal(page.elements.get('total-rate').textContent, `${pathname === '/' ? '0.00000040' : '0.00000060'}\u00a0BTC/FLM`);
    assert.deepEqual(rowValues(page), ['0.00002000', '50', '0.00001000', '0.00000060']);
    assert.deepEqual(footerValues(page), pathname === '/' ? ['0.00100000', '500', '0.00001000', '0.00000202'] : ['0.00002000', '50', '0.00001000', '0.00000060']);
    assert.equal(page.elements.get('total-reward').textContent, rewardBefore);
    assert.equal(page.elements.get('total-burned-label').textContent, 'Burned');
    assert.equal(page.elements.get('total-rate-label').textContent, pathname === '/' ? 'Current rate' : 'Effective rate');
    assert.match(page.elements.get('table-burned-label').textContent, /BTC/);
    assert.match(page.elements.get('table-fees-label').textContent, /BTC/);
    assert.match(page.elements.get('table-rate-label').textContent, /BTC\s*\/\s*FLM/);
    assert.equal(page.elements.get('chart-burn-unit').textContent, 'BTC/block');
    assert.equal(page.elements.get('chart-rate-unit').textContent, 'BTC/FLM');
    assert.match(page.elements.get('chart')['aria-label'], /BTC/);
    assert.equal(JSON.stringify(page.snapshot), snapshotBefore);
    assert.equal(stateRequests(page).length, 1);
  }
});

test('mint budget, fees, contribution labels, validation, and projection follow the unit without altering the draft', async () => {
  const page = await openPage('/mint');
  const budgetBefore = page.elements.get('budget').value;
  const projectedBefore = page.elements.get('projected-reward').textContent;
  await page.event('unit-btc', 'click');
  assert.equal(page.elements.get('budget-value').textContent, '0.01000000 BTC');
  assert.equal(page.elements.get('budget')['aria-valuetext'], '0.01000000 BTC');
  assert.equal(page.elements.get('budget-min').textContent, '0.00100000 BTC');
  assert.equal(page.elements.get('budget-max').textContent, '1.00000000 BTC');
  assert.equal(page.elements.get('projected-fees').textContent, '0.00500000 BTC');
  assert.equal(page.elements.get('projected-burn').textContent, '0.00500000 BTC');
  assert.match(page.elements.get('fee-label').textContent, /0\.00001000 BTC/);
  assert.match(page.elements.get('contributions').children[0].children[0].children[0].children[1].textContent, /0\.00300000 BTC/);
  const draft = { budget_sats: 1000000, duration: 500, start_block: 11 };
  const estimate = projectedReward(page.snapshot.contributions, draft, 'tf1demo-address', 10);
  assert.equal(page.elements.get('projected-rate').textContent, `${formatMoney(draft.budget_sats * SPARKS / estimate, 'BTC', null, true)} BTC / FLM`);
  assert.equal(page.elements.get('projected-reward').textContent, projectedBefore);
  assert.equal(page.elements.get('budget').value, budgetBefore);
  page.elements.get('budget').value = String(BUDGET_MIN);
  await page.event('budget', 'input');
  assert.equal(page.elements.get('submit').disabled, true);
  assert.match(page.elements.get('budget-error').textContent, /0\.00500500 BTC/);
  assert.doesNotMatch(page.elements.get('budget-error').textContent, /\bsat\b/);
});

test('USD mode reuses the public quote fetched without identity and converts all accounting displays, not accounting', async () => {
  for (const pathname of ['/', '/mint']) {
    const page = await openPage(pathname);
    const snapshotBefore = JSON.stringify(page.snapshot);
    await page.event('unit-usd', 'click');
    const requests = quoteRequests(page);
    assert.equal(requests.length, 1);
    const request = requests[0];
    assert.equal(request.url, 'https://api.coinbase.com/v2/prices/BTC-USD/spot');
    assert.equal(request.options.credentials, 'omit');
    assert.equal(request.options.referrerPolicy, 'no-referrer');
    assert.equal(request.options.body, undefined, 'wallet addresses and keys are never sent to the quote API');
    assert.ok(request.options.signal instanceof AbortSignal);
    assert.equal(page.abortTimeouts.length, 2, 'the snapshot and the quote each have a timeout');
    assert.ok(page.abortTimeouts.every(delay => delay === 10000), 'price requests cannot hang beyond ten seconds');
    assert.equal(page.elements.get('price-status').hidden, false);
    assert.match(page.elements.get('price-status').textContent, /50,000|50000/);
    assert.equal(page.elements.get('total-burned').textContent, pathname === '/' ? '$50.000' : '$1.000');
    assert.equal(page.elements.get('total-rate').textContent, `${pathname === '/' ? '0.02' : '0.03'}\u00a0$/FLM`);
    assert.deepEqual(rowValues(page), ['1.00', '50', '0.50', '0.03']);
    assert.deepEqual(footerValues(page), pathname === '/' ? ['50.00', '500', '0.50', '0.101'] : ['1.00', '50', '0.50', '0.03']);
    assert.equal(page.elements.get('chart-burn-unit').textContent, '$/block');
    assert.equal(page.elements.get('chart-rate-unit').textContent, '$/FLM');
    assert.equal(page.elements.get('total-rate-label').textContent, pathname === '/' ? 'Current rate' : 'Effective rate');
    if (pathname === '/mint') {
      assert.equal(page.elements.get('budget-value').textContent, '$500.00');
      assert.equal(page.elements.get('projected-fees').textContent, '$250.00');
      assert.equal(page.elements.get('projected-burn').textContent, '$250.00');
      assert.equal(page.elements.get('budget-min').textContent, '$50.00');
      assert.equal(page.elements.get('budget-max').textContent, '$50,000.00');
      assert.match(page.elements.get('projected-rate').textContent, /^\$[\d,.]+ \/ FLM$/);
    }
    assert.equal(JSON.stringify(page.snapshot), snapshotBefore);
    assert.equal(stateRequests(page).length, 1);
    assert.equal([...page.storage.values()].some(value => value === '50000'), false, 'quotes are not persisted');
  }
});

test('USD rates convert raw fractional sat/FLM rather than the truncated sat display', async () => {
  const page = await openPage('/mint');
  const contribution = page.snapshot.contributions[0];
  contribution.burned_sats = '199'; contribution.fees_sats = '0'; contribution.reward_sparks = '10000000000';
  await page.refresh();
  assert.equal(page.elements.get('total-rate').textContent, '1\u00a0sat/FLM');
  await page.event('unit-usd', 'click');
  assert.equal(page.elements.get('total-rate').textContent, '0.000995\u00a0$/FLM');
  assert.equal(rowValues(page)[3], '0.000995');
  assert.equal(footerValues(page)[3], '0.000995');
  assert.equal(page.elements.get('total-reward').textContent, '100\u00a0FLM');
});

test('top counters on both pages keep units without label prefixes and retain three truncated USD decimals across updates', async () => {
  for (const pathname of ['/', '/mint']) {
    let price = 50000;
    const page = await openPage(pathname, new Map(), undefined, undefined, { quote: () => quoteResponse(price) });
    page.snapshot.totals.burned_sats = '1999';
    page.snapshot.contributions[0].burned_sats = '1999';
    await page.refresh();
    const before = JSON.stringify(page.snapshot);
    assert.equal(page.elements.get('total-burned').textContent, '1,999\u00a0sat');
    await page.event('unit-btc', 'click');
    assert.equal(page.elements.get('total-burned').textContent, '0.00001999\u00a0btc');
    await page.event('unit-usd', 'click');
    assert.equal(page.elements.get('total-burned').textContent, '$0.999', 'top USD amounts truncate, rather than round, to exactly three places');
    assert.equal(rowValues(page)[0], '0.99', 'table precision is unchanged');
    assert.equal(page.elements.get('total-reward').textContent, `${pathname === '/' ? '500' : '50'}\u00a0FLM`);
    assert.match(page.elements.get('total-rate').textContent, /^[\d,.]+\u00a0\$\/FLM$/);
    assert.equal(JSON.stringify(page.snapshot), before, 'labels do not alter accounting');
    page.snapshot.totals.burned_sats = '2000';
    page.snapshot.contributions[0].burned_sats = '2000';
    await page.refresh();
    assert.equal(page.elements.get('total-burned').textContent, '$1.000');
    price = 100000;
    await page.timer(60000);
    assert.equal(page.elements.get('total-burned').textContent, '$2.000', 'quote refresh keeps the self-contained format');
    await page.event('unit-sat', 'click');
    assert.equal(page.elements.get('total-burned').textContent, '2,000\u00a0sat');
  }
});

test('unknown wallet totals and undefined rates keep their labels and selected units', async () => {
  const storage = new Map([['flame-mint-playground-wallet-v1', '{broken json']]);
  const page = await openPage('/mint', storage);
  for (const [unit, amount, rateUnit] of [['sat', '—\u00a0sat', 'sat'], ['btc', '—\u00a0btc', 'BTC'], ['usd', '$—', '$']]) {
    await page.event(`unit-${unit}`, 'click');
    await page.refresh();
    assert.equal(page.elements.get('total-burned').textContent, amount);
    assert.equal(page.elements.get('total-reward').textContent, '—\u00a0FLM');
    assert.equal(page.elements.get('total-rate').textContent, `—\u00a0${rateUnit}/FLM`);
  }
  for (const pathname of ['/', '/mint']) {
    const ready = await openPage(pathname);
    ready.snapshot.contributions = [];
    ready.snapshot.totals = { contribution_count: 0, burned_sats: '0', fees_sats: '0', reward_sparks: '0' };
    await ready.refresh();
    for (const [unit, amount, rateUnit] of [['sat', '0\u00a0sat', 'sat'], ['btc', '0.00000000\u00a0btc', 'BTC'], ['usd', '$0.000', '$']]) {
      await ready.event(`unit-${unit}`, 'click');
      assert.equal(ready.elements.get('total-burned').textContent, amount);
      assert.equal(ready.elements.get('total-reward').textContent, '0\u00a0FLM');
      assert.equal(ready.elements.get('total-rate').textContent, `—\u00a0${rateUnit}/FLM`);
    }
  }
});

test('both chart axes use selected currency while retaining the raw burn and fee scale', async () => {
  for (const pathname of ['/', '/mint']) {
    const page = await openPage(pathname);
    const ticks = () => page.draws.filter(call => call.canvas === page.elements.get('chart') && call.method === 'fillText' && call.args[2] !== 267);
    const parseAxis = text => Number(String(text).replaceAll(',', '').replace(/k$/, 'e3').replace(/m$/, 'e6'));
    const satTicks = ticks().filter(call => call.args[1] < 50).map(call => [parseAxis(call.args[0]), call.args[2]]);
    assert.ok(satTicks.some(([sats]) => sats > 0));
    if (pathname === '/') assert.ok(satTicks.some(([sats]) => sats < 0), 'the dashboard fee region is covered');
    for (const unit of ['BTC', 'USD']) {
      page.draws.length = 0;
      await page.event(`unit-${unit.toLowerCase()}`, 'click');
      const converted = ticks();
      for (const [sats, y] of satTicks) {
        const pair = converted.filter(call => call.args[2] === y);
        assert.equal(pair.length, 2, 'a shared horizontal line still has both vertical labels');
        assert.equal(pair[0].args[0], formatMoney(sats, unit, 50000));
        assert.equal(pair[1].args[0], formatMoney(sats / 50, unit, 50000, true));
      }
    }
  }
});

test('USD pending and failed quotes show unavailable amounts, never fictitious zeroes, while the snapshot stays live', async () => {
  let finish;
  const page = await openPage('/mint', new Map(), undefined, undefined, { quote: () => new Promise(resolve => { finish = resolve; }) });
  await page.event('unit-usd', 'click');
  assert.match(page.elements.get('price-status').textContent, /Loading/i);
  assert.equal(page.elements.get('total-burned').textContent, '$—');
  assert.equal(page.elements.get('total-rate').textContent, '—\u00a0$/FLM');
  for (const id of ['budget-value', 'projected-fees', 'projected-burn', 'projected-rate']) assert.equal(page.elements.get(id).textContent, '—', id);
  assert.deepEqual(rowValues(page), ['—', '50', '—', '—']);
  assert.equal(page.elements.get('total-reward').textContent, '50\u00a0FLM');
  finish({ ok: false, status: 503 });
  await page.settle();
  assert.match(page.elements.get('price-status').textContent, /unavailable|503/i);
  assert.equal(page.elements.get('total-burned').textContent, '$—');
  assert.equal(page.elements.get('connection').textContent, 'Live');
  await page.event('unit-btc', 'click');
  assert.equal(page.elements.get('total-burned').textContent, '0.00002000\u00a0btc');
  assert.equal(page.elements.get('price-status').hidden, false);
  assert.match(page.elements.get('price-status').textContent, /unavailable/i);
});

test('invalid, failed, and unreadable quotes leave USD unknown and retain functioning offline units', async () => {
  const variants = [
    () => { throw new Error('Network unavailable'); },
    () => ({ ok: true, json: async () => { throw new Error('Invalid JSON'); } }),
    ...['0', '-1', 'NaN'].map(amount => () => quoteResponse(amount)),
    () => ({ ok: true, json: async () => ({ data: { base: 'ETH', currency: 'USD', amount: '50000' } }) }),
  ];
  for (const quote of variants) {
    const page = await openPage('/', new Map(), undefined, undefined, { quote });
    await page.event('unit-usd', 'click');
    assert.equal(page.elements.get('total-burned').textContent, '$—');
    assert.match(page.elements.get('price-status').textContent, /unavailable/i);
    await page.event('unit-sat', 'click');
    assert.equal(page.elements.get('total-burned').textContent, '100,000\u00a0sat');
    assert.equal(page.elements.get('connection').textContent, 'Live');
  }
});

test('BTC/USD refreshes every minute in all units and discards stale values on failure', async () => {
  let count = 0;
  const page = await openPage('/', new Map(), undefined, undefined, { quote: () => ++count === 3 ? { ok: false, status: 503 } : quoteResponse(count * 50000) });
  await page.event('unit-usd', 'click');
  assert.equal(page.elements.get('total-burned').textContent, '$50.000');
  await page.timer(60000);
  assert.equal(page.elements.get('total-burned').textContent, '$100.000');
  assert.equal(quoteRequests(page).length, 2);
  await page.timer(60000);
  assert.equal(page.elements.get('total-burned').textContent, '$—', 'a failed refresh does not keep an apparently current old price');
  assert.match(page.elements.get('price-status').textContent, /unavailable/i);
  await page.event('unit-btc', 'click');
  assert.equal([...page.timers.values()].filter(timer => timer.delay === 60000).length, 1);
  assert.equal(page.elements.get('price-status').hidden, false);
  assert.equal(quoteRequests(page).length, 3, 'leaving USD does not fetch again');
  await page.timer(60000);
  assert.equal(quoteRequests(page).length, 4, 'the quote refreshes while BTC is selected');
  assert.equal(page.elements.get('total-burned').textContent, '0.00100000\u00a0btc');
  assert.match(page.elements.get('price-status').textContent, /200,000/);
  await page.event('unit-sat', 'click');
  await page.timer(60000);
  assert.equal(quoteRequests(page).length, 5, 'the quote refreshes while sat is selected');
  assert.equal(page.elements.get('total-burned').textContent, '100,000\u00a0sat');
  assert.equal(stateRequests(page).length, 1, 'quote refresh never fetches contribution history');
});

test('rapid unit changes reuse the in-flight quote and do not overlap or replace the selected unit', async () => {
  let finish;
  const page = await openPage('/', new Map(), undefined, undefined, { quote: () => new Promise(resolve => { finish = resolve; }) });
  await page.event('unit-usd', 'click');
  await page.event('unit-btc', 'click');
  await page.event('unit-usd', 'click');
  assert.equal(quoteRequests(page).length, 1);
  await page.event('unit-sat', 'click');
  finish(quoteResponse(50000));
  await page.settle();
  assert.equal(page.elements.get('unit-sat')['aria-pressed'], 'true');
  assert.equal(page.elements.get('total-burned').textContent, '100,000\u00a0sat');
  assert.equal(page.elements.get('price-status').hidden, false);
  assert.match(page.elements.get('price-status').textContent, /50,000/);
  assert.equal([...page.timers.values()].filter(timer => timer.delay === 60000).length, 1);
});

test('unit preferences persist, invalid values fall back to sats, and blocked storage never blocks the dashboard', async () => {
  for (const unit of ['sat', 'BTC', 'USD', 'invalid']) {
    const storage = new Map([[UNIT_KEY, unit]]);
    const page = await openPage('/', storage);
    assert.equal(page.elements.get(`unit-${unit === 'invalid' ? 'sat' : unit.toLowerCase()}`)['aria-pressed'], 'true');
    assert.equal(quoteRequests(page).length, 1, 'all initial units fetch the reference quote');
    assert.equal(storage.get(UNIT_KEY), unit, 'startup does not rewrite stored preferences or store quotes');
  }
  const page = await openPage('/', { get() { throw new Error('Storage blocked'); }, set() { throw new Error('Storage blocked'); } });
  assert.equal(page.elements.get('unit-sat')['aria-pressed'], 'true');
  await page.event('unit-btc', 'click');
  assert.equal(page.elements.get('total-burned').textContent, '0.00100000\u00a0btc');
  assert.equal(page.elements.get('connection').textContent, 'Live');
});

test('unit changes leave chart data, forecast, requests, and pending idempotence in satoshis', async () => {
  const storage = new Map();
  const page = await openPage('/mint', storage, async () => { throw new Error('Response lost'); });
  const projectedBefore = page.elements.get('projected-reward').textContent;
  const capture = async unit => {
    page.draws.length = 0;
    await page.event(`unit-${unit}`, 'click');
    return chartGeometry(page);
  };
  const satGeometry = await capture('sat');
  assert.deepEqual(await capture('btc'), satGeometry, 'chart heights retain raw-satoshi accounting');
  assert.deepEqual(await capture('usd'), satGeometry);
  assert.equal(page.elements.get('projected-reward').textContent, projectedBefore);
  assert.equal(page.elements.get('budget').value, '1000000');
  await page.event('contribution-form', 'submit');
  const request = JSON.parse(page.requests.find(request => request.url === '/api/contributions').options.body);
  assert.equal(request.budget_sats, 1000000);
  assert.equal(request.duration, 500);
  assert.deepEqual(Object.keys(request).sort(), ['address', 'budget_sats', 'delay_blocks', 'duration', 'request_id']);
  assert.deepEqual(JSON.parse(storage.get('flame-mint-playground-pending-v1')), request);
  const reloaded = await openPage('/mint', storage);
  assert.equal(reloaded.elements.get('unit-usd')['aria-pressed'], 'true');
  await reloaded.event('contribution-form', 'submit');
  assert.deepEqual(JSON.parse(reloaded.requests.find(request => request.url === '/api/contributions').options.body), request);
  assert.equal(storage.has('flame-mint-playground-pending-v1'), false);
});
