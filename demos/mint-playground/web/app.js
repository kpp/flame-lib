export const BUDGET_MIN = 100000;
export const BUDGET_MAX = 100000000;
export const BUDGET_STEP = 100000;
export const FEE = 1000;
export const SPARKS = 100000000;
export const REWARD = 50 * SPARKS;
export const UNIT_KEY = 'flame-mint-playground-unit-v1';
export const TIME_KEY = 'flame-mint-playground-time-v1';
export const ZOOM_KEY = 'flame-mint-playground-zoom-v2';
export const ZOOM_MIN = 10;
export const ZOOM_DEFAULT = 500;
export const BLOCKS_PER_DAY = 144;
export const DURATION_MIN = 144;
export const DURATION_MAX = 4464;
export const DELAY_MAX = 4464;
const SATS_PER_BTC = 100000000;

// Typed-value fields next to the sliders. A typed number is parsed leniently
// (currency signs, unit words and thousands separators are ignored), converted
// to whole sats or blocks independently of the slider's range. The slider
// only displays the nearest available step. `null` means nothing usable was typed.
export function parseInputNumber(text) {
  const cleaned = String(text ?? '').replace(/[^0-9.]/g, '');
  if (!cleaned || cleaned === '.') return null;
  const value = Number(cleaned);
  return Number.isFinite(value) ? value : null;
}

export function snapToRange(value, min, max, step = 1) {
  if (value === null || !Number.isFinite(value)) return null;
  const snapped = Math.round(value / step) * step;
  return Math.min(max, Math.max(min, snapped));
}

export function inputToSats(value, unit = 'sat', btcUsd = null) {
  if (value === null) return null;
  if (unit === 'sat') return value;
  if (unit === 'BTC') return value * SATS_PER_BTC;
  if (!(Number.isFinite(btcUsd) && btcUsd > 0)) return null;
  return value / btcUsd * SATS_PER_BTC;
}

export function inputToBlocks(value, mode = 'blocks') {
  if (value === null) return null;
  return mode === 'days' ? value * BLOCKS_PER_DAY : value;
}

export function blockDate(block) {
  if (block < 0) return '—';
  const date = new Date(Date.UTC(2027, 0, 11) + Math.floor((block - 1) / BLOCKS_PER_DAY) * 86400000);
  if (Number.isNaN(date.getTime())) return `Block ${new Intl.NumberFormat('en').format(block)}`;
  return new Intl.DateTimeFormat('en', { month: 'short', day: 'numeric', timeZone: 'UTC' }).format(date);
}

export function formatDurationValue(blocks, mode = 'blocks') {
  const value = mode === 'days' ? blocks / BLOCKS_PER_DAY : blocks;
  return new Intl.NumberFormat('en', { maximumFractionDigits: 3 }).format(value);
}

export function durationUnit(blocks, mode = 'blocks') {
  const value = mode === 'days' ? blocks / BLOCKS_PER_DAY : blocks;
  return `${mode === 'days' ? 'day' : 'block'}${value === 1 ? '' : 's'}`;
}

export function formatDuration(blocks, mode = 'blocks') {
  return `${formatDurationValue(blocks, mode)} ${durationUnit(blocks, mode)}`;
}

export function formatMoney(sats, unit = 'sat', btcUsd = null, isRate = false, digits = null) {
  if (unit === 'USD' && !(Number.isFinite(btcUsd) && btcUsd > 0)) return '—';
  const value = unit === 'BTC' ? sats / SATS_PER_BTC : unit === 'USD' ? sats * (btcUsd / SATS_PER_BTC) : sats;
  const minimumFractionDigits = digits ?? (unit === 'BTC' ? 8 : unit === 'USD' ? 2 : 0);
  return new Intl.NumberFormat('en', { minimumFractionDigits, maximumFractionDigits: unit === 'USD' && isRate ? 6 : minimumFractionDigits, roundingMode: 'trunc' }).format(value);
}

export function parseBtcUsdQuote(quote) {
  const data = quote?.data;
  if (data?.base !== 'BTC' || data?.currency !== 'USD' || typeof data.amount !== 'string' || !data.amount.trim()) throw new Error('Invalid BTC/USD quote');
  const price = Number(data.amount);
  if (!Number.isFinite(price) || price <= 0) throw new Error('Invalid BTC/USD quote');
  return price;
}

// Give the earliest blocks the remainder, matching the integer database schedule.
export function burnAt(contribution, block) {
  const offset = block - contribution.start_block;
  if (offset < 0 || offset >= contribution.duration) return 0;
  const burn = contribution.budget_sats - contribution.duration * FEE;
  return Math.floor(burn / contribution.duration) + (offset < burn % contribution.duration ? 1 : 0);
}

export function feeAt(contribution, block) {
  return block >= contribution.start_block && block < contribution.start_block + contribution.duration ? FEE : 0;
}

export function chartBlocks(contributions, firstBlock, lastBlock, heightNow, pending = null) {
  const count = lastBlock - firstBlock + 1;
  if (count <= 2001) return Array.from({ length: count }, (_, index) => firstBlock + index);
  // Rates only change at schedule boundaries; keep both sides so even one-block spikes survive zooming out.
  const blocks = new Set([firstBlock, lastBlock]);
  const add = block => { if (block >= firstBlock && block <= lastBlock) blocks.add(block); };
  add(heightNow);
  for (const fraction of [.25, .5, .75]) add(firstBlock + (lastBlock - firstBlock) * fraction);
  const schedule = contribution => {
    const burn = contribution.budget_sats - contribution.duration * FEE;
    const remainder = burn % contribution.duration;
    const boundaries = [contribution.start_block, contribution.start_block + contribution.duration];
    if (remainder > 0) boundaries.push(contribution.start_block + remainder);
    for (const block of boundaries) { add(block - 1); add(block); }
  };
  contributions.forEach(schedule);
  if (pending) schedule(pending);
  return [...blocks].sort((left, right) => left - right);
}

export function projectedReward(contributions, draft) {
  const start = draft.start_block;
  const end = start + draft.duration;
  const events = new Map();
  const add = (block, change, own) => {
    block = Math.max(start, Math.min(end, block));
    const event = events.get(block) ?? { total: 0n, own: 0n };
    event.total += change;
    if (own) event.own += change;
    events.set(block, event);
  };
  const schedule = (contribution, own = false) => {
    const duration = BigInt(contribution.duration);
    const burn = BigInt(contribution.budget_sats) - duration * BigInt(FEE);
    const rate = burn / duration;
    const remainder = Number(burn % duration);
    add(contribution.start_block, rate + (remainder > 0 ? 1n : 0n), own);
    if (remainder > 0) add(contribution.start_block + remainder, -1n, own);
    add(contribution.start_block + contribution.duration, -rate, own);
  };
  schedule(draft, true);
  contributions.forEach(contribution => schedule(contribution));

  // Burn rates only change at schedule boundaries; keep sums exact before computing shares.
  let reward = 0;
  let previous = start;
  let total = 0n;
  let own = 0n;
  for (const [block, event] of [...events].sort(([left], [right]) => left - right)) {
    if (total > 0n) reward += REWARD * Number(own) / Number(total) * (block - previous);
    total += event.total;
    own += event.own;
    previous = block;
  }
  return reward;
}

// Originals plus RGB midpoints, ordered for maximum minimum adjacent OKLab distance, including wrap.
export const PALETTE = ['#ffd800', '#ff6922', '#ffc200', '#ff4242', '#ff9c00', '#ffe400', '#ff9921', '#fff000', '#ffaf00', '#ff5632', '#ffcd00', '#ff8311'];

export function userColor(userId) {
  return PALETTE[userId % PALETTE.length];
}

export function contributionStatus(contribution, height) {
  if (height < contribution.start_block) return 'scheduled';
  return height < contribution.start_block + contribution.duration - 1 ? 'active' : 'finished';
}

if (typeof document !== 'undefined') {
  const element = id => document.getElementById(id);
  const minting = location.pathname.replace(/\/$/, '') === '/mint';
  const number = (value, digits = 0) => new Intl.NumberFormat('en', { maximumFractionDigits: digits }).format(value);
  const whole = value => number(Math.trunc(value) || 0);
  let unit = 'sat';
  let timeMode = 'blocks';
  let zoom = ZOOM_DEFAULT;
  try {
    const saved = localStorage.getItem(UNIT_KEY);
    if (['sat', 'BTC', 'USD'].includes(saved)) unit = saved;
  } catch { /* Unit selection also works without browser storage. */ }
  try {
    const saved = localStorage.getItem(TIME_KEY);
    if (['blocks', 'days'].includes(saved)) timeMode = saved;
  } catch { /* Time selection also works without browser storage. */ }
  try {
    const saved = Number(localStorage.getItem(ZOOM_KEY));
    if (Number.isSafeInteger(saved) && saved >= ZOOM_MIN) zoom = saved;
  } catch { /* Zoom also works without browser storage. */ }
  let btcUsd = null;
  let quoteRequest = false;
  let quoteStatus = 'Loading BTC/USD…';
  const unitLabel = () => unit === 'USD' ? '$' : unit;
  const money = (sats, isRate = false) => formatMoney(sats, unit, btcUsd, isRate);
  const amount = sats => {
    const value = money(sats);
    return value === '—' ? value : unit === 'USD' ? `$${value}` : `${value} ${unit}`;
  };
  const rate = (sats, reward) => reward > 0 ? money(sats * SPARKS / reward, true) : '—';
  const rateAmount = (sats, reward) => {
    const value = rate(sats, reward);
    return value === '—' ? value : unit === 'USD' ? `$${value} / FLM` : `${value} ${unit} / FLM`;
  };
  const metricAmount = sats => {
    const value = sats === null ? '—' : formatMoney(sats, unit, btcUsd, false, unit === 'USD' ? 3 : null);
    return unit === 'USD' ? `$${value}` : `${value}\u00a0${unit.toLowerCase()}`;
  };
  const metricReward = sparks => `${sparks === null ? '—' : whole(sparks / SPARKS)}\u00a0FLM`;
  const metricRate = (sats, reward) => `${rate(sats, reward)}\u00a0${unitLabel()}/FLM`;
  const walletKey = 'flame-mint-playground-wallet-v1';
  const pendingKey = 'flame-mint-playground-pending-v1';
  let state = null;
  let wallet = null;
  let submitting = false;
  let pendingRequest = null;
  let brokenPending = false;
  let lastReceived = 0;
  let chartQueued = false;
  let feeFill = null;

  for (const color of PALETTE) {
    const swatch = document.createElement('span');
    swatch.style.background = color;
    swatch.title = color;
    swatch.setAttribute('role', 'listitem');
    swatch.setAttribute('aria-label', color);
    element('palette-preview').append(swatch);
  }
  element('empty-table').textContent = minting ? 'No contributions.' : 'No recent contributions.';
  if (minting) {
    document.title = 'Mint';
    element('page-title').textContent = 'Mint';
    element('navigation').href = '/';
    element('navigation').textContent = 'Dashboard';
    element('total-rate-label').textContent = 'Effective rate';
    element('total-rate-label').title = 'Your burned coins divided by your FLM earned, excluding mining fees';
    element('mint-form').hidden = false;
    element('wallet-panel').hidden = false;
    element('own-legend').hidden = false;
    element('draft-legend').hidden = false;
    element('fees-legend').hidden = true;
    element('chart-wrap').className = 'canvas-wrap';
    element('chart-title').textContent = 'Burn rate';
    element('others-label').textContent = 'Others';
    element('contributions-title').textContent = 'Your contributions';
    try {
      const saved = JSON.parse(localStorage.getItem(pendingKey) || 'null');
      if (saved) {
        const legacyDuration = Number.isInteger(saved.duration) && saved.duration >= 100 && saved.duration <= 1000 && saved.duration % 50 === 0;
        const currentDuration = Number.isSafeInteger(saved.duration) && saved.duration >= DURATION_MIN;
        const delay = saved.delay_blocks === undefined ? 0 : saved.delay_blocks;
        if (typeof saved.address !== 'string' || typeof saved.request_id !== 'string' || !Number.isSafeInteger(saved.budget_sats) || saved.budget_sats < BUDGET_MIN || !(legacyDuration || currentDuration) || !Number.isSafeInteger(delay) || delay < 0) throw new Error('Invalid saved request');
        pendingRequest = { address: saved.address, budget_sats: saved.budget_sats, duration: saved.duration, delay_blocks: delay, request_id: saved.request_id };
        element('budget').value = String(snapToRange(saved.budget_sats, BUDGET_MIN, BUDGET_MAX, BUDGET_STEP));
        element('duration').value = String(snapToRange(saved.duration, DURATION_MIN, DURATION_MAX));
        element('delay').value = String(snapToRange(delay, 0, DELAY_MAX));
        element('submit-message').textContent = 'Saved request unconfirmed. Retry safely with the same settings.';
      }
    } catch {
      brokenPending = true;
      element('submit-message').textContent = 'Saved request unreadable. Discard it before submitting again.';
    }
  }

  // Exact typed values are independent of the clamped slider positions.
  const typedValues = {
    budget: pendingRequest?.budget_sats ?? null,
    duration: pendingRequest ? Math.max(DURATION_MIN, pendingRequest.duration) : null,
    delay: pendingRequest?.delay_blocks ?? null,
  };
  const draft = () => {
    const delay = pendingRequest?.delay_blocks ?? typedValues.delay ?? Number(element('delay').value);
    return { budget_sats: pendingRequest?.budget_sats ?? typedValues.budget ?? Number(element('budget').value), duration: pendingRequest?.duration ?? typedValues.duration ?? Number(element('duration').value), delay_blocks: delay, start_block: (state?.height ?? 0) + 1 + delay };
  };
  const validDraft = contribution => [contribution.budget_sats, contribution.duration, contribution.delay_blocks, contribution.start_block].every(Number.isSafeInteger)
    && contribution.budget_sats >= BUDGET_MIN && contribution.duration >= (pendingRequest ? 100 : DURATION_MIN) && contribution.delay_blocks >= 0
    && contribution.start_block <= Number.MAX_SAFE_INTEGER - contribution.duration
    && contribution.budget_sats >= contribution.duration * (FEE + 1);
  const typedField = id => element(`${id}-value`);
  // Keep a field the user is editing untouched: the page re-renders every second.
  const showTyped = (id, text, unitText) => {
    element(`${id}-unit`).textContent = unitText;
    const field = typedField(id);
    if (typeof document !== 'undefined' && document.activeElement === field) return;
    field.value = text;
  };
  const sliderBounds = { budget: [BUDGET_MIN, BUDGET_MAX, BUDGET_STEP], duration: [DURATION_MIN, DURATION_MAX, 1], delay: [0, DELAY_MAX, 1] };
  function applyTyped(id) {
    const slider = element(id);
    const parsed = parseInputNumber(typedField(id).value);
    const native = id === 'budget' ? inputToSats(parsed, unit, btcUsd) : inputToBlocks(parsed, timeMode);
    const exact = snapToRange(native, sliderBounds[id][0], Number.MAX_SAFE_INTEGER);
    if (exact === null || slider.disabled) { updateDraft(); return; }
    typedValues[id] = exact;
    slider.value = String(snapToRange(exact, ...sliderBounds[id]));
    updateDraft(true);
  }

  function updateDraft(resetRequest = false) {
    if (resetRequest && !pendingRequest) element('submit-message').textContent = '';
    const contribution = draft();
    const valid = validDraft(contribution);
    showTyped('budget', money(contribution.budget_sats), unit);
    showTyped('duration', formatDurationValue(contribution.duration, timeMode), durationUnit(contribution.duration, timeMode));
    showTyped('delay', formatDurationValue(contribution.delay_blocks, timeMode), durationUnit(contribution.delay_blocks, timeMode));
    element('budget').setAttribute('aria-valuetext', amount(Number(element('budget').value)));
    for (const id of ['duration', 'delay']) {
      const blocks = Number(element(id).value);
      element(id).setAttribute('aria-valuetext', `${formatDuration(blocks, timeMode)} (${blocks} blocks)`);
    }
    element('projection-note').textContent = `Known contribution schedules included. Starts ${timeMode === 'days' ? blockDate(contribution.start_block) : `at block ${number(contribution.start_block)}`}.`;
    element('budget-error').textContent = valid ? '' : contribution.budget_sats < contribution.duration * (FEE + 1)
      ? `Minimum ${amount(contribution.duration * (FEE + 1))} (${amount(contribution.duration * FEE)} fees + ${amount(1)} burned per block). Increase budget or shorten duration.`
      : 'Values and the end block must not exceed 9,007,199,254,740,991.';
    const canEstimate = state && wallet && valid;
    const estimate = canEstimate ? projectedReward(state.contributions, contribution) : 0;
    element('projected-reward').textContent = canEstimate ? whole(estimate / SPARKS) : '—';
    element('projected-fees').textContent = amount(contribution.duration * FEE);
    element('projected-burn').textContent = valid ? amount(contribution.budget_sats - contribution.duration * FEE) : '—';
    element('projected-rate').textContent = estimate ? rateAmount(contribution.budget_sats - contribution.duration * FEE, estimate) : '—';
    const locked = submitting || !!pendingRequest || brokenPending;
    for (const id of ['budget', 'duration', 'delay']) element(id).disabled = typedField(id).disabled = locked;
    element('discard-pending').hidden = !pendingRequest && !brokenPending;
    element('discard-pending').disabled = submitting;
    element('submit').disabled = !valid || !wallet || !state || submitting || brokenPending || Date.now() - lastReceived > 5000;
    element('submit').textContent = submitting ? 'Submitting…' : !wallet ? 'Wallet not ready' : pendingRequest ? 'Retry pending' : 'Submit';
    queueChart();
  }

  async function prepareWallet() {
    element('retry-wallet').hidden = true;
    element('reset-wallet').hidden = true;
    element('wallet-status').textContent = 'Wallet loading…';
    try {
      const stored = JSON.parse(localStorage.getItem(walletKey) || 'null');
      if (stored && (typeof stored.address !== 'string' || typeof stored.mnemonic !== 'string')) throw new Error('The saved wallet is unreadable');
      const load = async () => {
        const { default: init, generateMnemonic, Wallet } = await import('/wallet/flamewallet_wasm.js');
        await init();
        const mnemonic = stored ? stored.mnemonic : generateMnemonic(12);
        const keys = Wallet.fromMnemonic(mnemonic, '', 'testnet', 0);
        try {
          const address = keys.nextAddress().address;
          if (stored && stored.address !== address) throw new Error('The saved address does not match the recovery phrase');
          return { mnemonic, address };
        } finally { keys.free(); }
      };
      let timeout;
      try {
        wallet = await Promise.race([load(), new Promise((_, reject) => {
          timeout = setTimeout(() => reject(new Error('The browser wallet did not finish loading.')), 15000);
        })]);
      } finally { clearTimeout(timeout); }
      // Saving must succeed before accepting contributions for this identity.
      localStorage.setItem(walletKey, JSON.stringify(wallet));
      element('wallet-address').textContent = wallet.address;
      element('wallet-status').textContent = 'Saved locally. Keys stay in this browser.';
    } catch (error) {
      wallet = null;
      element('wallet-status').textContent = `Wallet unavailable: ${error.message || String(error)}. Check that the wallet assets were built and browser storage is enabled.`;
      element('retry-wallet').hidden = false;
      element('reset-wallet').hidden = false;
    }
    updateDraft();
    if (state) renderTable();
  }

  function renderTable() {
    const contributions = state.contributions.filter(contribution => !minting || contribution.address === wallet?.address).slice().sort((a, b) => b.id - a.id);
    const rows = document.createDocumentFragment();
    let burned = 0, fees = 0, rewards = 0;
    for (const contribution of contributions) {
      const row = document.createElement('tr');
      const identity = document.createElement('td');
      const heading = document.createElement('div');
      heading.className = 'contribution-identity';
      const dot = document.createElement('i');
      dot.className = 'participant-dot';
      dot.style.background = userColor(contribution.user_id);
      const title = document.createElement('span');
      title.className = 'contribution-id';
      title.textContent = `#${contribution.id} · ${amount(contribution.budget_sats)} / ${formatDuration(contribution.duration, timeMode)}`;
      heading.append(dot, title);
      const address = document.createElement('p');
      address.className = 'address-label';
      const end = contribution.start_block + contribution.duration - 1;
      const schedule = timeMode === 'days' ? `${blockDate(contribution.start_block)}–${blockDate(end)}` : `Blocks ${number(contribution.start_block)}–${number(end)}`;
      address.textContent = schedule;
      address.title = minting ? contribution.address : '';
      identity.append(heading, address);
      row.append(identity);
      const statusCell = document.createElement('td');
      const status = document.createElement('span');
      const statusName = contributionStatus(contribution, state.height);
      status.className = `status ${statusName}`;
      status.textContent = statusName[0].toUpperCase() + statusName.slice(1);
      statusCell.append(status);
      row.append(statusCell);
      const burn = Number(contribution.burned_sats), fee = Number(contribution.fees_sats), reward = Number(contribution.reward_sparks);
      for (const value of [money(burn), whole(reward / SPARKS), money(fee), rate(burn, reward)]) {
        const cell = document.createElement('td'); cell.className = 'number'; cell.textContent = value; row.append(cell);
      }
      burned += burn; fees += fee; rewards += reward;
      rows.append(row);
    }
    element('contributions').replaceChildren(rows);
    element('empty-table').hidden = contributions.length > 0;
    if (!minting) {
      burned = Number(state.totals.burned_sats);
      fees = Number(state.totals.fees_sats);
      rewards = Number(state.totals.reward_sparks);
    }
    const totalRow = document.createElement('tr');
    const count = minting ? contributions.length : state.totals.contribution_count;
    for (const [index, value] of [minting ? 'Total' : 'All-time total', `${count} contributions`, money(burned), whole(rewards / SPARKS), money(fees), rate(burned, rewards)].entries()) {
      const cell = document.createElement('td'); if (index > 1) cell.className = 'number'; cell.textContent = value; totalRow.append(cell);
    }
    element('contribution-totals').replaceChildren(totalRow);
    if (minting) {
      element('total-burned').textContent = metricAmount(wallet ? burned : null);
      element('total-fees').textContent = metricAmount(wallet ? fees : null);
      element('total-reward').textContent = metricReward(wallet ? rewards : null);
      element('total-rate').textContent = metricRate(burned, wallet ? rewards : 0);
    }
  }

  function renderMoney() {
    const label = unitLabel();
    for (const value of ['sat', 'BTC', 'USD']) element(`unit-${value.toLowerCase()}`).setAttribute('aria-pressed', String(unit === value));
    element('price-status').textContent = quoteStatus;
    element('table-burned-label').textContent = unit === 'sat' ? 'Sats burned' : `${label} burned`;
    element('table-fees-label').textContent = `Mining fees (${label})`;
    element('table-rate-label').textContent = `${label} / FLM`;
    element('chart-burn-unit').textContent = `${label}/block`;
    element('chart-rate-unit').textContent = `${label}/FLM`;
    const spokenUnit = unit === 'sat' ? 'satoshis' : unit === 'USD' ? 'US dollars' : 'BTC';
    element('chart').setAttribute('aria-label', minting
      ? `Burn rate in ${spokenUnit} per block on the left; mint rate in ${spokenUnit} per FLM on the right. Previous and next ${zoom} blocks, shown as ${timeMode === 'days' ? 'calendar dates' : 'block numbers'}. All known contribution schedules included.`
      : `Burn above zero and mining fees below zero share a ${spokenUnit}-per-block scale on the left. Right-hand values are divided by 50 FLM. Previous and next ${zoom} blocks, shown as ${timeMode === 'days' ? 'calendar dates' : 'block numbers'}. All known contribution schedules included.`);
    element('zoom-in').disabled = zoom === ZOOM_MIN;
    element('zoom-out').disabled = zoom === Number.MAX_SAFE_INTEGER;
    for (const value of ['blocks', 'days']) element(`time-${value}`).setAttribute('aria-pressed', String(timeMode === value));
    element('current-block-label').textContent = timeMode === 'days' ? 'Date' : 'Block';
    element('current-block').textContent = state ? timeMode === 'days' ? blockDate(state.height) : number(state.height) : '—';
    element('chart-window').textContent = `±${formatDuration(zoom, timeMode)}`;
    element('duration-min').textContent = formatDuration(DURATION_MIN, timeMode);
    element('duration-max').textContent = formatDuration(DURATION_MAX, timeMode);
    element('delay-min').textContent = formatDuration(0, timeMode);
    element('delay-max').textContent = formatDuration(DELAY_MAX, timeMode);
    element('budget-min').textContent = amount(BUDGET_MIN);
    element('budget-max').textContent = amount(BUDGET_MAX);
    element('fee-label').textContent = `Fee: ${amount(FEE)}/block`;
    element('projected-rate-label').textContent = `${label}/FLM`;
    if (state) {
      renderTable();
      if (!minting) {
        element('total-burned').textContent = metricAmount(Number(state.totals.burned_sats));
        element('total-fees').textContent = metricAmount(Number(state.totals.fees_sats));
        element('total-reward').textContent = metricReward(Number(state.totals.reward_sparks));
        const currentBurn = state.contributions.reduce((sum, contribution) => sum + burnAt(contribution, state.height), 0);
        element('total-rate').textContent = metricRate(currentBurn, currentBurn ? REWARD : 0);
      }
    } else {
      element('total-burned').textContent = metricAmount(null);
      element('total-fees').textContent = metricAmount(null);
      element('total-reward').textContent = metricReward(null);
      element('total-rate').textContent = metricRate(0, 0);
    }
    updateDraft();
  }

  async function refreshQuote() {
    if (quoteRequest) return;
    quoteRequest = true;
    if (btcUsd === null) {
      quoteStatus = 'Loading BTC/USD…';
      renderMoney();
    }
    try {
      const response = await fetch('https://api.coinbase.com/v2/prices/BTC-USD/spot', { credentials: 'omit', referrerPolicy: 'no-referrer', cache: 'no-store', signal: AbortSignal.timeout(10000) });
      if (!response.ok) throw new Error(`Price service returned ${response.status}`);
      btcUsd = parseBtcUsdQuote(await response.json());
      quoteStatus = `BTC/USD $${formatMoney(SATS_PER_BTC, 'USD', btcUsd)} · Coinbase`;
    } catch {
      btcUsd = null;
      quoteStatus = 'BTC/USD unavailable. Retrying in 60 seconds.';
    } finally {
      quoteRequest = false;
      renderMoney();
      setTimeout(refreshQuote, 60000);
    }
  }

  function selectUnit(value) {
    unit = value;
    try { localStorage.setItem(UNIT_KEY, value); } catch { /* Unit selection still applies in this page. */ }
    renderMoney();
  }

  function selectTime(value) {
    timeMode = value;
    try { localStorage.setItem(TIME_KEY, value); } catch { /* Time selection still applies in this page. */ }
    renderMoney();
  }

  function changeZoom(direction) {
    const next = Math.max(ZOOM_MIN, Math.min(Number.MAX_SAFE_INTEGER, direction < 0 ? Math.round(zoom / 2) : zoom * 2));
    if (next === zoom) return;
    zoom = next;
    try { localStorage.setItem(ZOOM_KEY, String(zoom)); } catch { /* Zoom still applies in this page. */ }
    renderMoney();
  }

  function queueChart() {
    if (chartQueued) return;
    chartQueued = true;
    requestAnimationFrame(() => { chartQueued = false; drawChart(); });
  }

  function drawChart() {
    const canvas = element('chart');
    const width = canvas.clientWidth, height = canvas.clientHeight;
    if (!width || !height) return;
    const ratio = window.devicePixelRatio || 1;
    canvas.width = Math.round(width * ratio); canvas.height = Math.round(height * ratio);
    const context = canvas.getContext('2d');
    context.scale(ratio, ratio);
    const side = unit === 'sat' ? 48 : unit === 'BTC' ? 94 : 82;
    const margin = { left: side, top: 14, right: side, bottom: 27 };
    const w = width - margin.left - margin.right, h = height - margin.top - margin.bottom;
    const heightNow = state?.height ?? 0, firstBlock = heightNow - zoom, lastBlock = heightNow + zoom;
    const pending = draft();
    const showDraft = minting && wallet && validDraft(pending);
    const contributions = state?.contributions || [];
    const blocks = chartBlocks(contributions, firstBlock, lastBlock, heightNow, showDraft ? pending : null);
    const count = blocks.length;
    const colored = !minting;
    const groups = new Map();
    const fees = new Float64Array(count);
    const ownContribution = contributions.find(contribution => contribution.address === wallet?.address);
    const ownColor = ownContribution ? userColor(ownContribution.user_id) : '#888';
    if (minting) {
      element('own-swatch').style.background = ownColor;
      element('draft-swatch').style.borderColor = ownColor;
    }
    const ordered = minting ? contributions : contributions.slice().sort((a, b) => a.id - b.id);
    for (const contribution of ordered) {
      const own = minting && contribution.address === wallet?.address;
      const key = own ? 'own' : colored ? contribution.user_id : 'other';
      const values = groups.get(key) || new Float64Array(count);
      // Move the whole user band to the top on each newer contribution.
      if (colored) groups.delete(key);
      groups.set(key, values);
      for (let i = 0; i < count; i++) {
        const block = blocks[i];
        values[i] += burnAt(contribution, block);
        if (!minting) fees[i] += feeAt(contribution, block);
      }
    }
    const ownValues = groups.get('own');
    if (ownValues) { groups.delete('own'); groups.set('own', ownValues); }
    const totals = new Float64Array(count);
    for (const values of groups.values()) for (let i = 0; i < count; i++) totals[i] += values[i];
    let maximum = 0, maximumFees = 0;
    for (let i = 0; i < count; i++) {
      maximum = Math.max(maximum, totals[i] + (showDraft ? burnAt(pending, blocks[i]) : 0));
      maximumFees = Math.max(maximumFees, fees[i]);
    }
    const minimum = minting ? 0 : -maximumFees;
    const roughStep = Math.max(1, maximum - minimum) / 5;
    const power = 10 ** Math.floor(Math.log10(roughStep));
    const step = Math.max(1, [1, 2, 5, 10].find(factor => factor * power >= roughStep) * power);
    const high = Math.max(step, Math.ceil(maximum / step) * step);
    const low = Math.floor(minimum / step) * step;
    const xBlock = block => margin.left + (block - firstBlock) / (lastBlock - firstBlock) * w;
    const x = i => xBlock(blocks[i]);
    const nowX = xBlock(heightNow);
    const y = value => margin.top + (high - value) / (high - low) * h;
    const zero = y(0);
    const axisNumber = value => unit !== 'sat' ? money(value) : Math.abs(value) >= 1000000 ? `${number(value / 1000000, 2)}m` : Math.abs(value) >= 1000 ? `${number(value / 1000, 2)}k` : number(value, 2);
    context.font = '10px -apple-system, sans-serif';
    context.textBaseline = 'middle';
    for (let tick = Math.round(low / step); tick <= Math.round(high / step); tick++) {
      const value = tick * step;
      context.strokeStyle = '#e5e5e5'; context.lineWidth = 1;
      context.beginPath(); context.moveTo(margin.left, y(value)); context.lineTo(margin.left + w, y(value)); context.stroke();
      context.fillStyle = '#666'; context.textAlign = 'right';
      context.fillText(axisNumber(value), margin.left - 9, y(value));
      context.textAlign = 'left';
      context.fillText(money(value / (REWARD / SPARKS), true), width - margin.right + 8, y(value));
    }
    if (!minting) {
      if (!feeFill) {
        const tile = document.createElement('canvas');
        tile.width = tile.height = 8;
        const hatch = tile.getContext('2d');
        hatch.fillStyle = '#f2f2f2'; hatch.fillRect(0, 0, 8, 8);
        hatch.strokeStyle = '#888';
        hatch.beginPath(); hatch.moveTo(0, 8); hatch.lineTo(8, 0); hatch.stroke();
        feeFill = context.createPattern(tile, 'repeat');
      }
      context.beginPath(); context.moveTo(x(0), zero);
      for (let i = 0; i < count; i++) context.lineTo(x(i), y(-fees[i]));
      context.lineTo(x(count - 1), zero); context.closePath();
      context.fillStyle = feeFill; context.fill();
    }
    const base = new Float64Array(count);
    for (const [key, values] of groups) {
      context.beginPath(); context.moveTo(x(0), y(base[0]));
      for (let i = 0; i < count; i++) context.lineTo(x(i), y(base[i] + values[i]));
      for (let i = count - 1; i >= 0; i--) context.lineTo(x(i), y(base[i]));
      context.closePath(); context.fillStyle = key === 'own' ? ownColor : key === 'other' ? '#a3a3a3' : userColor(key); context.globalAlpha = key === 'other' ? .76 : 1; context.fill();
      for (let i = 0; i < count; i++) base[i] += values[i];
    }
    context.globalAlpha = 1;
    context.fillStyle = '#ffffff30'; context.fillRect(nowX, margin.top, w / 2, h);
    if (!minting) {
      context.strokeStyle = '#666';
      context.beginPath(); context.moveTo(x(0), zero); context.lineTo(x(count - 1), zero); context.stroke();
    }
    context.strokeStyle = '#666'; context.setLineDash([3, 4]); context.beginPath(); context.moveTo(nowX, margin.top); context.lineTo(nowX, margin.top + h); context.stroke(); context.setLineDash([]);
    if (showDraft) {
      const start = Math.max(firstBlock, pending.start_block);
      const end = Math.min(lastBlock, pending.start_block + pending.duration - 1);
      if (start <= end) {
        const from = Math.max(firstBlock, start - 1), to = Math.min(lastBlock, end + 1);
        let started = false;
        context.beginPath();
        for (let i = 0; i < count; i++) {
          if (blocks[i] < from || blocks[i] > to) continue;
          const value = totals[i] + burnAt(pending, blocks[i]);
          if (!started) context.moveTo(x(i), y(value)); else context.lineTo(x(i), y(value));
          started = true;
        }
        context.strokeStyle = ownColor; context.lineWidth = 1.8; context.setLineDash([5, 4]); context.stroke(); context.setLineDash([]);
      }
    }
    context.fillStyle = '#666'; context.font = '10px -apple-system, sans-serif';
    for (const fraction of [0, .25, .5, .75, 1]) {
      context.textAlign = fraction === 0 ? 'left' : fraction === 1 ? 'right' : 'center';
      const block = firstBlock + (lastBlock - firstBlock) * fraction;
      context.fillText(block < 0 ? '—' : timeMode === 'days' ? blockDate(block) : number(block), xBlock(block), height - 8);
    }
    if (!state) { context.textAlign = 'center'; context.fillText('Loading…', margin.left + w / 2, margin.top + h / 2); }
  }

  async function poll() {
    try {
      const response = await fetch(minting ? '/api/state?full_history=true' : '/api/state', { cache: 'no-store', signal: AbortSignal.timeout(10000) });
      if (!response.ok) throw new Error(`Server returned ${response.status}`);
      state = await response.json();
      lastReceived = Date.now();
      element('connection').className = 'connection';
      element('connection').textContent = 'Live';
      renderMoney();
    } catch (error) {
      element('connection').className = 'connection error';
      element('connection').textContent = `Disconnected${state ? ' (last snapshot)' : ''}. ${error.message}. Retrying…`;
      if (minting) element('submit').disabled = true;
    } finally { setTimeout(poll, 1000); }
  }

  for (const value of ['sat', 'BTC', 'USD']) element(`unit-${value.toLowerCase()}`).addEventListener('click', () => selectUnit(value));
  for (const value of ['blocks', 'days']) element(`time-${value}`).addEventListener('click', () => selectTime(value));
  element('zoom-in').addEventListener('click', () => changeZoom(-1));
  element('zoom-out').addEventListener('click', () => changeZoom(1));
  for (const id of ['budget', 'duration', 'delay']) {
    element(id).addEventListener('input', () => { typedValues[id] = null; updateDraft(true); });
    const field = typedField(id);
    field.addEventListener('change', () => applyTyped(id));
    // Enter applies the value instead of submitting the form.
    field.addEventListener('keydown', event => {
      if (event.key !== 'Enter') return;
      event.preventDefault();
      applyTyped(id);
      field.blur?.();
    });
    field.addEventListener('focus', () => field.select?.());
  }
  element('retry-wallet').addEventListener('click', prepareWallet);
  element('reset-wallet').addEventListener('click', () => {
    if (!confirm('Forget this saved demo wallet and create a new one? Save its recovery phrase first if it is available. Existing rewards will remain linked to the old address.')) return;
    try { localStorage.removeItem(walletKey); prepareWallet(); }
    catch (error) { element('wallet-status').textContent = error.message; }
  });
  element('discard-pending').addEventListener('click', () => {
    if (submitting || !confirm('Discard this pending request? It might already have been accepted. Discarding does not cancel it, and submitting again could create a second contribution. Retry instead if you are unsure.')) return;
    try {
      localStorage.removeItem(pendingKey); pendingRequest = null; brokenPending = false;
      element('submit-message').textContent = 'Pending request discarded. Check your contributions before submitting again.';
      updateDraft();
    } catch (error) { element('submit-message').textContent = error.message; }
  });
  element('seed-details').addEventListener('toggle', () => {
    element('seed-phrase').textContent = element('seed-details').open ? wallet?.mnemonic || 'Wallet is not available.' : '';
  });
  element('contribution-form').addEventListener('submit', async event => {
    event.preventDefault();
    if (submitting || brokenPending || !wallet || !state || !validDraft(draft()) || Date.now() - lastReceived > 5000) return;
    submitting = true; updateDraft();
    for (const id of ['budget', 'duration', 'delay']) element(id).disabled = typedField(id).disabled = true;
    element('submit-message').className = 'form-message';
    element('submit-message').textContent = '';
    try {
      if (!pendingRequest) {
        const contribution = draft();
        const request = { address: wallet.address, budget_sats: contribution.budget_sats, duration: contribution.duration, delay_blocks: contribution.delay_blocks, request_id: crypto.randomUUID() };
        localStorage.setItem(pendingKey, JSON.stringify(request));
        pendingRequest = request;
      }
      const response = await fetch('/api/contributions', { method: 'POST', headers: { 'Content-Type': 'application/json' }, signal: AbortSignal.timeout(10000), body: JSON.stringify(pendingRequest) });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || `Server returned ${response.status}`);
      element('submit-message').className = 'form-message success';
      element('submit-message').textContent = `#${result.id} accepted. Starts ${timeMode === 'days' ? blockDate(result.start_block) : `at block ${number(result.start_block)}`}.`;
      localStorage.removeItem(pendingKey);
      pendingRequest = null;
    } catch (error) {
      element('submit-message').className = 'form-message';
      element('submit-message').textContent = `${error.message}. Request saved locally; retry safely after reloading.`;
    } finally {
      submitting = false; updateDraft();
    }
  });
  new ResizeObserver(queueChart).observe(element('chart'));
  renderMoney();
  refreshQuote();
  if (minting) prepareWallet();
  poll();
}
