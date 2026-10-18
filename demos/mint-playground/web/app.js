export const BUDGET_MIN = 100000;
export const BUDGET_MAX = 100000000;
export const FEE = 1000;
export const SPARKS = 100000000;
export const REWARD = 50 * SPARKS;

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

export function projectedReward(contributions, draft) {
  let reward = 0;
  for (let offset = 0; offset < draft.duration; offset++) {
    const block = draft.start_block + offset;
    const own = burnAt(draft, block);
    const total = contributions.reduce((sum, contribution) => sum + burnAt(contribution, block), own);
    if (total > 0) reward += REWARD * own / total;
  }
  return reward;
}

export const PALETTE = ['#ffd800', '#ffc200', '#ff9c00', '#ff6922', '#ff4242', '#fff000'];

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
  const rate = (sats, reward) => reward > 0 ? whole(sats * SPARKS / reward) : '—';
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

  element('empty-table').textContent = minting ? 'No contributions.' : 'No recent contributions.';
  if (minting) {
    document.title = 'Mint';
    element('page-title').textContent = 'Mint';
    element('navigation').href = '/';
    element('navigation').textContent = 'Dashboard';
    element('total-rate-label').textContent = 'Effective sat/FLM';
    element('total-rate-label').title = 'Your sats burned plus mining fees paid, divided by your FLM earned';
    element('mint-form').hidden = false;
    element('own-legend').hidden = false;
    element('draft-legend').hidden = false;
    element('fees-legend').hidden = true;
    element('chart-wrap').className = 'canvas-wrap';
    element('chart-title').textContent = 'Burn rate';
    element('chart').setAttribute('aria-label', 'Burn rate in satoshis per block on the left; mint rate in satoshis per FLM on the right. Previous and next 500 blocks.');
    element('others-label').textContent = 'Others';
    element('contributions-title').textContent = 'Your contributions';
    try {
      const saved = JSON.parse(localStorage.getItem(pendingKey) || 'null');
      if (saved) {
        if (typeof saved.address !== 'string' || typeof saved.request_id !== 'string' || !Number.isSafeInteger(saved.budget_sats) || saved.budget_sats < BUDGET_MIN || saved.budget_sats > BUDGET_MAX || !Number.isInteger(saved.duration) || saved.duration < 100 || saved.duration > 1000 || saved.duration % 50 !== 0) throw new Error('Invalid saved request');
        pendingRequest = { address: saved.address, budget_sats: saved.budget_sats, duration: saved.duration, request_id: saved.request_id };
        element('budget').value = saved.budget_sats;
        element('duration').value = saved.duration;
        element('submit-message').textContent = 'Saved request unconfirmed. Retry safely with the same settings.';
      }
    } catch {
      brokenPending = true;
      element('submit-message').textContent = 'Saved request unreadable. Discard it before submitting again.';
    }
  }

  const draft = () => ({ budget_sats: pendingRequest?.budget_sats ?? Number(element('budget').value), duration: Number(element('duration').value), start_block: (state?.height ?? 0) + 1 });
  const validDraft = contribution => contribution.budget_sats >= contribution.duration * (FEE + 1);

  function updateDraft(resetRequest = false) {
    if (resetRequest && !pendingRequest) element('submit-message').textContent = '';
    const contribution = draft();
    const valid = validDraft(contribution);
    element('budget-value').textContent = `${number(contribution.budget_sats)} sat`;
    element('duration-value').textContent = `${number(contribution.duration)} blocks`;
    element('budget').setAttribute('aria-valuetext', `${number(contribution.budget_sats)} satoshis`);
    element('duration').setAttribute('aria-valuetext', `${contribution.duration} blocks, about ${number(contribution.duration / 60, 1)} minutes`);
    element('budget-error').textContent = valid ? '' : `Minimum ${number(contribution.duration * (FEE + 1))} sat (${number(contribution.duration * FEE)} fees + 1 sat burned per block). Increase budget or shorten duration.`;
    const estimate = state && valid ? projectedReward(state.contributions, contribution) : 0;
    element('projected-reward').textContent = state && valid ? whole(estimate / SPARKS) : '—';
    element('projected-fees').textContent = `${number(contribution.duration * FEE)} sat`;
    element('projected-burn').textContent = valid ? `${number(contribution.budget_sats - contribution.duration * FEE)} sat` : '—';
    element('projected-rate').textContent = estimate ? `${rate(contribution.budget_sats, estimate)} sat / FLM` : '—';
    element('budget').disabled = element('duration').disabled = submitting || !!pendingRequest || brokenPending;
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
      title.textContent = `#${contribution.id} · ${number(contribution.budget_sats)} sat / ${contribution.duration} blocks`;
      heading.append(dot, title);
      const address = document.createElement('p');
      address.className = 'address-label';
      address.textContent = minting ? `Blocks ${contribution.start_block}–${contribution.start_block + contribution.duration - 1}` : `${contribution.address.slice(0, 13)}…${contribution.address.slice(-7)}`;
      address.title = contribution.address;
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
      for (const value of [number(burn), whole(reward / SPARKS), number(fee), rate(burn + fee, reward)]) {
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
    for (const [index, value] of [minting ? 'Total' : 'All-time total', `${count} contributions`, number(burned), whole(rewards / SPARKS), number(fees), rate(burned + fees, rewards)].entries()) {
      const cell = document.createElement('td'); if (index > 1) cell.className = 'number'; cell.textContent = value; totalRow.append(cell);
    }
    element('contribution-totals').replaceChildren(totalRow);
    if (minting) {
      element('total-burned').textContent = wallet ? number(burned) : '—';
      element('total-reward').textContent = wallet ? whole(rewards / SPARKS) : '—';
      element('total-rate').textContent = wallet ? rate(burned + fees, rewards) : '—';
    }
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
    const margin = { left: 48, top: 14, right: 48, bottom: 27 };
    const w = width - margin.left - margin.right, h = height - margin.top - margin.bottom;
    const halfWindow = 500;
    const heightNow = state?.height ?? 0, firstBlock = heightNow - halfWindow, count = 2 * halfWindow + 1;
    const colored = !minting;
    const groups = new Map();
    const fees = new Float64Array(count);
    const contributions = state?.contributions || [];
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
      const start = Math.max(firstBlock, contribution.start_block);
      const end = Math.min(firstBlock + count, contribution.start_block + contribution.duration);
      for (let block = start; block < end; block++) {
        const i = block - firstBlock;
        values[i] += burnAt(contribution, block);
        if (!minting) fees[i] += feeAt(contribution, block);
      }
    }
    const ownValues = groups.get('own');
    if (ownValues) { groups.delete('own'); groups.set('own', ownValues); }
    const totals = new Float64Array(count);
    for (const values of groups.values()) for (let i = 0; i < count; i++) totals[i] += values[i];
    const pending = draft();
    const showDraft = minting && validDraft(pending);
    let maximum = 0;
    for (let i = 0; i < count; i++) maximum = Math.max(maximum, totals[i] + (showDraft ? burnAt(pending, firstBlock + i) : 0));
    const minimum = minting ? 0 : -Math.max(0, ...fees);
    const roughStep = Math.max(1, maximum - minimum) / 5;
    const power = 10 ** Math.floor(Math.log10(roughStep));
    const step = Math.max(1, [1, 2, 5, 10].find(factor => factor * power >= roughStep) * power);
    const high = Math.max(step, Math.ceil(maximum / step) * step);
    const low = Math.floor(minimum / step) * step;
    const x = i => margin.left + i / (count - 1) * w;
    const y = value => margin.top + (high - value) / (high - low) * h;
    const zero = y(0);
    const axisNumber = value => Math.abs(value) >= 1000000 ? `${number(value / 1000000, 2)}m` : Math.abs(value) >= 1000 ? `${number(value / 1000, 2)}k` : number(value, 2);
    context.font = '10px -apple-system, sans-serif';
    context.textBaseline = 'middle';
    for (let tick = Math.round(low / step); tick <= Math.round(high / step); tick++) {
      const value = tick * step;
      context.strokeStyle = '#e5e5e5'; context.lineWidth = 1;
      context.beginPath(); context.moveTo(margin.left, y(value)); context.lineTo(margin.left + w, y(value)); context.stroke();
      context.fillStyle = '#666'; context.textAlign = 'right';
      context.fillText(axisNumber(value), margin.left - 9, y(value));
      context.textAlign = 'left';
      context.fillText(whole(value / (REWARD / SPARKS)), width - margin.right + 8, y(value));
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
    context.fillStyle = '#ffffff30'; context.fillRect(x(halfWindow), margin.top, w / 2, h);
    if (!minting) {
      context.strokeStyle = '#666';
      context.beginPath(); context.moveTo(x(0), zero); context.lineTo(x(count - 1), zero); context.stroke();
    }
    context.strokeStyle = '#666'; context.setLineDash([3, 4]); context.beginPath(); context.moveTo(x(halfWindow), margin.top); context.lineTo(x(halfWindow), margin.top + h); context.stroke(); context.setLineDash([]);
    if (showDraft) {
      context.beginPath();
      for (let i = halfWindow; i < count; i++) {
        const block = firstBlock + i;
        if (block > pending.start_block + pending.duration) break;
        const value = totals[i] + burnAt(pending, block);
        if (i === halfWindow) context.moveTo(x(i), y(value)); else context.lineTo(x(i), y(value));
      }
      context.strokeStyle = ownColor; context.lineWidth = 1.8; context.setLineDash([5, 4]); context.stroke(); context.setLineDash([]);
    }
    context.fillStyle = '#666'; context.font = '10px -apple-system, sans-serif';
    for (const i of [0, halfWindow / 2, halfWindow, 3 * halfWindow / 2, count - 1]) {
      context.textAlign = i === 0 ? 'left' : i === count - 1 ? 'right' : 'center';
      context.fillText(firstBlock + i < 0 ? '—' : number(firstBlock + i), x(i), height - 8);
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
      if (!minting) {
        element('total-burned').textContent = number(Number(state.totals.burned_sats));
        element('total-reward').textContent = whole(Number(state.totals.reward_sparks) / SPARKS);
        const currentBurn = state.contributions.reduce((sum, contribution) => sum + burnAt(contribution, state.height), 0);
        element('total-rate').textContent = currentBurn ? whole(currentBurn / (REWARD / SPARKS)) : '—';
      }
      element('current-block').textContent = number(state.height);
      renderTable(); updateDraft();
    } catch (error) {
      element('connection').className = 'connection error';
      element('connection').textContent = `Disconnected${state ? ' (last snapshot)' : ''}. ${error.message}. Retrying…`;
      if (minting) element('submit').disabled = true;
    } finally { setTimeout(poll, 1000); }
  }

  element('budget').addEventListener('input', () => updateDraft(true));
  element('duration').addEventListener('input', () => updateDraft(true));
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
    element('budget').disabled = true; element('duration').disabled = true;
    element('submit-message').className = 'form-message';
    element('submit-message').textContent = '';
    try {
      if (!pendingRequest) {
        const contribution = draft();
        const request = { address: wallet.address, budget_sats: contribution.budget_sats, duration: contribution.duration, request_id: crypto.randomUUID() };
        localStorage.setItem(pendingKey, JSON.stringify(request));
        pendingRequest = request;
      }
      const response = await fetch('/api/contributions', { method: 'POST', headers: { 'Content-Type': 'application/json' }, signal: AbortSignal.timeout(10000), body: JSON.stringify(pendingRequest) });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || `Server returned ${response.status}`);
      element('submit-message').className = 'form-message success';
      element('submit-message').textContent = `#${result.id} accepted. Starts at block ${result.start_block}.`;
      localStorage.removeItem(pendingKey);
      pendingRequest = null;
    } catch (error) {
      element('submit-message').className = 'form-message';
      element('submit-message').textContent = `${error.message}. Request saved locally; retry safely after reloading.`;
    } finally {
      submitting = false; element('budget').disabled = false; element('duration').disabled = false; updateDraft();
    }
  });
  new ResizeObserver(queueChart).observe(element('chart'));
  updateDraft();
  if (minting) prepareWallet();
  poll();
}
