import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const source = await readFile(new URL('./app.js', import.meta.url), 'utf8');
const { BUDGET_MIN, BUDGET_MAX, burnAt, feeAt, projectedReward, PALETTE, userColor, contributionStatus, FEE, SPARKS } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
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
  const contribution = plan({ budget_sats: 300000 });
  assert.equal(projectedReward([], contribution), 5000 * SPARKS);
  assert.equal(projectedReward([contribution], contribution), 2500 * SPARKS);
});

test('projection accounts for a competitor ending halfway through a plan', () => {
  const draft = plan({ budget_sats: 300000 });
  const competing = plan({ budget_sats: 150000, duration: 50 });
  assert.equal(projectedReward([competing], draft), 3750 * SPARKS);
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

test('linear slider budgets conserve the complete budget at every allowed duration', () => {
  for (const budget_sats of [BUDGET_MIN, 200000, 500000, 1000000, 50000000, BUDGET_MAX]) {
    for (let duration = 100; duration <= 1000; duration += 50) {
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

async function openPage(pathname, storage = new Map(), post = async () => ({ id: 2, user_id: 1, start_block: 11 }), walletReady = Promise.resolve()) {
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
      } });
    }
    get clientWidth() { return 800; }
    get clientHeight() { return 275; }
  }
  const elements = new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([, id]) => [id, new Element()]));
  elements.get('budget').value = '1000000'; elements.get('duration').value = '500';
  const requests = [], frames = [], timers = new Map();
  let nextTimer = 0;
  const snapshot = { height: 10, totals: { contribution_count: 1, burned_sats: '100000', fees_sats: '1000', reward_sparks: '50000000000' }, contributions: [{ id: 1, user_id: 1, address: 'tf1demo-address', budget_sats: 300000, duration: 100, start_block: 10, burned_sats: '2000', fees_sats: '1000', reward_sparks: '5000000000' }] };
  const context = vm.createContext({
    document: { getElementById: id => elements.get(id), createElement: tag => new Element(tag), createDocumentFragment: () => new Element('fragment') },
    location: { pathname }, window: { devicePixelRatio: 1 }, AbortSignal, console,
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
      return { ok: true, json: async () => post(JSON.parse(options.body)) };
    },
  });
  const script = source.replace(/^export /gm, '').replace("await import('/wallet/flamewallet_wasm.js')", 'await Promise.resolve(walletModule)');
  vm.runInContext(script, context);
  const settle = async () => { for (let i = 0; i < 4; i++) await new Promise(resolve => setImmediate(resolve)); while (frames.length) frames.shift()(); };
  await settle();
  return {
    elements, requests, storage, snapshot, draws, settle,
    event: async (id, event) => { await elements.get(id).handlers[event]({ preventDefault() {} }); await settle(); },
    refresh: async () => {
      const [id, timer] = [...timers].find(([, timer]) => timer.delay === 1000);
      timers.delete(id);
      await timer.callback();
      await settle();
    },
  };
}

test('both charts span exactly 500 blocks before and after the current block', async () => {
  const html = await readFile(new URL('./index.html', import.meta.url), 'utf8');
  assert.match(html, /±500 blocks/);
  assert.match(html, /aria-label="[^"]*Previous and next 500 blocks\./);
  for (const pathname of ['/', '/mint']) {
    const page = await openPage(pathname);
    page.snapshot.height = 500;
    page.snapshot.contributions = [plan({ id: 1, address: 'tf1demo-address', start_block: 300, duration: 1000, budget_sats: 3000000 })];
    page.draws.length = 0;
    await page.refresh();
    const draws = page.draws.filter(call => call.canvas === page.elements.get('chart'));
    const ticks = draws.filter(call => call.method === 'fillText' && call.args[2] === 267);
    assert.deepEqual(ticks.map(call => call.args[0]), ['0', '250', '500', '750', '1,000']);
    assert.deepEqual(ticks.map(call => call.args[1]), [48, 224, 400, 576, 752]);
    const shading = draws.find(call => call.method === 'fillRect' && call.fillStyle === '#ffffff30');
    assert.equal(shading.args[0], 400, 'future shading begins at the centered current block');
    assert.equal(shading.args[2], 352, 'future shading ends at the right edge');
    const pathBefore = index => {
      const start = draws.slice(0, index).findLastIndex(call => call.method === 'beginPath');
      return draws.slice(start + 1, index).filter(call => call.method === 'moveTo' || call.method === 'lineTo');
    };
    const band = pathBefore(draws.findIndex(call => call.method === 'fill' && call.fillStyle === userColor(1)));
    assert.equal(band.length, 2003, 'a closed band contains 1,001 top and 1,001 bottom samples plus its starting point');
    assert.equal(Math.min(...band.map(call => call.args[0])), 48);
    assert.equal(Math.max(...band.map(call => call.args[0])), 752);
    if (pathname === '/mint') {
      assert.match(page.elements.get('chart')['aria-label'], /Previous and next 500 blocks\./);
      const draft = pathBefore(draws.findIndex(call => call.method === 'stroke' && call.strokeStyle === userColor(1)));
      assert.equal(draft.length, 501, 'draft sampling stays within the visible future window');
      assert.equal(draft[0].args[0], 400);
      assert.equal(draft.at(-1).args[0], 752);
    }
  }
});

test('dashboard burn and patterned downward fees share one sats scale, with mint rates on the right', async () => {
  const page = await openPage('/');
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
  const pattern = page.draws.find(call => call.method === 'fill' && call.fillStyle?.kind === 'pattern').fillStyle;
  assert.equal(pattern.repetition, 'repeat');
  const strokes = page.draws.filter(call => call.canvas === pattern.canvas);
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
  const tint = page.draws.find(call => call.method === 'fillRect' && call.canvas === page.elements.get('chart'));
  assert.equal(tint.fillStyle, '#ffffff30', 'future tint is light enough to preserve saturated colors');
  assert.ok(tint.args[1] + tint.args[3] >= baseline + oneDepth - 1e-10, 'future shading covers fees too');
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
  assert.equal(page.elements.get('total-burned').textContent, '100,000');
  assert.equal(page.elements.get('total-reward').textContent, '500');
  assert.equal(page.elements.get('total-rate').textContent, '40');
  assert.equal(page.elements.get('current-block').textContent, '10');
  assert.equal(page.elements.get('connection').textContent, 'Live');
  assert.equal(page.elements.get('contributions').children.length, 1);
  assert.equal(page.requests.length, 1);
  assert.equal(page.storage.size, 0, 'dashboard does not need palette or wallet storage');
});

test('dashboard requests recent contributions but uses server all-time totals, even with no visible rows', async () => {
  const page = await openPage('/');
  page.snapshot.totals = { contribution_count: 42, burned_sats: '9876543', fees_sats: '1234567', reward_sparks: '123456789999' };
  await page.refresh();
  const footer = () => page.elements.get('contribution-totals').children[0].children.map(cell => cell.textContent);
  assert.equal(page.elements.get('contributions').children.length, 1, 'only the returned recent row is displayed');
  assert.deepEqual(footer(), ['All-time total', '42 contributions', '9,876,543', '1,234', '1,234,567', '8,999']);
  assert.equal(page.elements.get('total-burned').textContent, '9,876,543');
  assert.equal(page.elements.get('total-reward').textContent, '1,234');
  page.snapshot.contributions = [];
  page.draws.length = 0;
  await page.refresh();
  assert.equal(page.elements.get('contributions').children.length, 0);
  assert.equal(page.elements.get('empty-table').hidden, false);
  assert.equal(page.elements.get('empty-table').textContent, 'No recent contributions.');
  assert.ok(!page.draws.some(call => call.method === 'fill' && PALETTE.includes(call.fillStyle)), 'omitted contributions create no chart bands');
  assert.deepEqual(footer(), ['All-time total', '42 contributions', '9,876,543', '1,234', '1,234,567', '8,999']);
  assert.ok(page.requests.every(request => request.url === '/api/state'), 'dashboard never requests full history');
});

test('mint requests full history and retains personal totals for contributions finished more than 10,000 blocks ago', async () => {
  const page = await openPage('/mint');
  page.snapshot.height = 20000;
  page.snapshot.contributions[0].reward_sparks = '199999999';
  await page.refresh();
  assert.ok(page.requests.every(request => request.url === '/api/state?full_history=true'));
  assert.equal(page.elements.get('contributions').children.length, 1);
  assert.equal(page.elements.get('contributions').children[0].children[1].children[0].textContent, 'Finished');
  assert.equal(page.elements.get('total-burned').textContent, '2,000');
  assert.equal(page.elements.get('total-reward').textContent, '1');
  assert.equal(page.elements.get('total-rate').textContent, '1,500');
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
    assert.equal(page.elements.get('total-reward').textContent, '1');
    assert.equal(page.elements.get('total-rate').textContent, pathname === '/mint' ? '999' : '40');
    if (pathname === '/mint') {
      const draft = { budget_sats: 1000000, duration: 500, start_block: page.snapshot.height + 1 };
      const estimate = projectedReward(page.snapshot.contributions, draft);
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
  assert.deepEqual(totals(), ['122,345', '133', '1,795']);
  assert.equal(page.elements.get('total-rate-label').textContent, 'Effective sat/FLM');
  assert.match(page.elements.get('total-rate-label').title, /mining fees/);
  const table = page.elements.get('contribution-totals').children[0].children.map(cell => cell.textContent);
  assert.deepEqual(table.slice(2), ['122,345', '133', '117,000', '1,795'], 'header and table truncate only displayed reward and cost');
  assert.deepEqual(page.elements.get('contributions').children.map(row => row.children[1].children[0].textContent), ['Scheduled', 'Active', 'Finished']);
  page.elements.get('budget').value = String(BUDGET_MAX);
  await page.event('budget', 'input');
  page.elements.get('duration').value = '1000';
  await page.event('duration', 'input');
  assert.deepEqual(totals(), ['122,345', '133', '1,795'], 'draft budgets and projected rewards never change the accrued header');
});

test('mint effective cost is undefined before any FLM is earned, and a ready unregistered wallet has zero totals', async () => {
  const page = await openPage('/mint');
  page.snapshot.contributions[0].reward_sparks = '0';
  await page.refresh();
  assert.equal(page.elements.get('total-burned').textContent, '2,000');
  assert.equal(page.elements.get('total-reward').textContent, '0');
  assert.equal(page.elements.get('total-rate').textContent, '—');
  page.snapshot.contributions[0].address = 'another-user';
  await page.refresh();
  assert.equal(page.elements.get('total-burned').textContent, '0');
  assert.equal(page.elements.get('total-reward').textContent, '0');
  assert.equal(page.elements.get('total-rate').textContent, '—');
});

test('an unavailable wallet never substitutes global totals on the mint page', async () => {
  const storage = new Map([['flame-mint-playground-wallet-v1', '{broken json']]);
  const page = await openPage('/mint', storage);
  for (const id of ['total-burned', 'total-reward', 'total-rate']) assert.equal(page.elements.get(id).textContent, '—');
  await page.refresh();
  for (const id of ['total-burned', 'total-reward', 'total-rate']) assert.equal(page.elements.get(id).textContent, '—');
  assert.equal(page.elements.get('contributions').children.length, 0);
});

test('a wallet arriving after the first poll fills personal totals without waiting for another poll', async () => {
  let finishWallet;
  const walletReady = new Promise(resolve => { finishWallet = resolve; });
  const page = await openPage('/mint', new Map(), undefined, walletReady);
  assert.equal(page.elements.get('connection').textContent, 'Live', 'the snapshot arrived while wallet initialization is pending');
  for (const id of ['total-burned', 'total-reward', 'total-rate']) assert.equal(page.elements.get(id).textContent, '—');
  finishWallet();
  await page.settle();
  assert.equal(page.elements.get('total-burned').textContent, '2,000');
  assert.equal(page.elements.get('total-reward').textContent, '50');
  assert.equal(page.elements.get('total-rate').textContent, '60');
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
  const bands = (page, colors = PALETTE) => {
    let path = [];
    const result = [];
    for (const call of page.draws.filter(call => call.canvas === page.elements.get('chart'))) {
      if (call.method === 'beginPath') path = [];
      if (call.method === 'moveTo' || call.method === 'lineTo') path.push(call.args);
      if (call.method === 'fill' && colors.includes(call.fillStyle)) {
        const [top, base] = path.filter(point => point[0] === 400).map(point => point[1]);
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
  const ownAndOthers = bands(mint, ['#a3a3a3', userColor(1)]);
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

test('dashboard colors do not access browser storage', async () => {
  const page = await openPage('/', { get() { assert.fail('Unexpected storage read'); }, set() { assert.fail('Unexpected storage write'); } });
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
  assert.equal(page.elements.get('total-rate').textContent, '40', 'future contribution does not affect this block');
  page.snapshot.height = 11;
  await page.refresh();
  assert.equal(page.elements.get('total-rate').textContent, '60', '3,000 sats burned this block / 50 FLM');
  page.snapshot.height = 110;
  await page.refresh();
  assert.equal(page.elements.get('total-rate').textContent, '20', 'first contribution no longer burns');
  page.snapshot.height = 111;
  await page.refresh();
  assert.equal(page.elements.get('total-rate').textContent, '—', 'no issuance in an empty block');
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
  assert.equal(first.elements.get('seed-phrase').textContent, undefined);
  await first.event('contribution-form', 'submit');
  const original = JSON.parse(first.requests.find(request => request.url === '/api/contributions').options.body);
  assert.deepEqual(Object.keys(original).sort(), ['address', 'budget_sats', 'duration', 'request_id']);
  assert.equal(first.elements.get('budget').disabled, true);
  assert.equal(first.elements.get('duration').disabled, true);
  assert.ok(storage.has('flame-mint-playground-pending-v1'));
  const reloaded = await openPage('/mint', storage);
  assert.equal(reloaded.elements.get('budget').disabled, true);
  assert.equal(reloaded.elements.get('submit').textContent, 'Retry pending');
  await reloaded.event('contribution-form', 'submit');
  assert.deepEqual(JSON.parse(reloaded.requests.find(request => request.url === '/api/contributions').options.body), original);
  assert.equal(storage.has('flame-mint-playground-pending-v1'), false);
  assert.equal(reloaded.elements.get('budget').disabled, false);
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
  assert.deepEqual(JSON.parse(page.requests.find(request => request.url === '/api/contributions').options.body), pending);
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
