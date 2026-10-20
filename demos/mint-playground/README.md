# Mint playground

Multi-player minting simulator: a Rust web service, SQLite ledger, and a small HTML/JavaScript UI. Satoshis and rewards are simulated. It records rewards by Flame receiving address; **it does not create real UTXOs or accept Bitcoin payments**.

## Run

From this directory:

```sh
./run.sh
```

Open [the dashboard](http://127.0.0.1:3000/) or [minting page](http://127.0.0.1:3000/mint). The script builds the existing `flamewallet-wasm` browser wallet and starts the service. It needs the repository's Rust toolchain, the `wasm32-unknown-unknown` target, and `wasm-bindgen-cli` matching the repository's `Cargo.lock`. If the CLI is missing, the script prints an installation command. Browser wallet generation needs a secure context: localhost works; a remote deployment needs HTTPS.

The default database is `data/mint.sqlite3` in this directory. To choose a persistent file and a different listening address:

```sh
MINT_LISTEN=127.0.0.1:3001 ./run.sh /absolute/path/mint.sqlite3
```

Once browser-wallet assets have been built, `cargo run --locked` starts just the service. A binary deployed away from this source tree finds them through `MINT_WALLET_DIR`, the directory holding `flamewallet_wasm.js` and `flamewallet_wasm_bg.wasm`; the [container image](Dockerfile) sets it, along with `MINT_LISTEN`. This is a standalone crate with its own lockfile; it does not change the protocol workspace's dependencies.

```sh
cargo test --locked
node --test web/test.mjs
```

## How it works

User chooses: 
- budget: amount of satoshis to spend total,
- duration: how many blocks to spend it over,
- delay: how many blocks to wait before starting.

Each contribution starts at `committed_height + 1 + delay_blocks`. Delay defaults to zero (the next block). Its slider covers 0 to 4,464 blocks; the duration slider covers 144 to 4,464 blocks, both at single-block resolution. Typed values may exceed these slider ranges. Waiting blocks incur no burn, fee, or reward. The total budget includes a fee of 1,000 sat **per contribution, per active block**. The remaining sats are burned across the selected duration: each block burns the integer quotient, and the first remainder blocks burn one extra sat. Thus the complete budget is spent exactly, without fractional sats.

The budget slider runs linearly from 100,000 to 100,000,000 sat, in steps of 100,000. The adjacent text fields accept larger budgets, durations and delays independently of the slider positions; typed budgets retain single-satoshi precision. Dragging a slider replaces only that field's typed value. Currency/time switches and refreshes preserve the underlying sats/blocks. Inputs are bounded by JavaScript's exact integer ceiling, `9,007,199,254,740,991`; the exclusive end block must also fit. Submission requires `budget >= duration × 1,001`: enough for all fees and at least one burned sat per block. Invalid combinations are explained and cannot be submitted. Sliders retain their original limits.

Blocks advance once per second while the service is running. Restarting resumes at the last committed height; downtime does not produce blocks or retroactive rewards. A block with at least one active minter distributes exactly 50 FLM in proportion to each contribution's burned sats, excluding fees. With no active minters, that block's 50 FLM are permanently lost: never accumulated, carried forward, or awarded to later minters. Scheduled contributions that have not started and expired contributions do not count as active. One FLM is 100,000,000 sparks. Allocation uses integer arithmetic: floor each exact share, then assign leftover sparks to largest fractional remainders, breaking ties by contribution ID. Charts and estimates use every known contribution's actual start and expiry, including future scheduled contributions; unknown future submissions are not predicted.

The blocks / days switch changes display only and is remembered independently of currency. Days use 144 blocks/day: block 1 begins January 11, 2027, and block 145 begins January 12. Calendar labels use UTC (`Jan 11`, `Mar 13`); durations and delays are shown in days while controls, requests, and storage retain integer blocks. This is an accelerated simulation calendar, not wall-clock scheduling.

Both pages have − / + chart zoom controls. The centered half-window is 144, 432, 720, …, 4,464 blocks (1, 3, 5, …, 31 days). `+` zooms in by 288 blocks on each side; `−` zooms out. The initial window is ±432 blocks. Boundary buttons are disabled, and the selected zoom is remembered independently of time and currency display. Zoom changes only the chart, not contribution settings or estimates.


The existing Flame wallet runs in the browser via WebAssembly and uses FlameKD to generate a mnemonic and testnet receiving address (`t1…i1…`; see [address formatting](../../docs/address.md)). These are remembered in browser local storage, so subsequent contributions share the same reward destination. **The mnemonic, seed and private keys never go to the service or database.** Request parsing rejects extra fields, including mnemonic/private-key fields. Only the address and contribution parameters are submitted. The server validates the address with FlameKD and stores its canonical lowercase encoding.

Local storage is only suitable for this demo. Never reuse this mnemonic for real funds; clearing browser data loses it, and scripts running on the same origin can read it. The server cannot recover it. Address identity is public attribution, not authentication: this simulation does not prove address ownership, debit a real balance, or prevent someone submitting simulated contributions to another address. Keep the default loopback binding unless intentionally sharing the demo. A real paid deployment needs authentication, payment verification and abuse controls before accepting contributions.

## Data model

One dedicated thread owns the SQLite connection. HTTP requests and block ticks go through its bounded command queue; it never shares a connection behind a mutex or blocks an async worker with SQL. SQLite foreign keys, strict integer columns and checked arithmetic protect accounting. Block processing is one transaction: block record, contribution accruals and accumulated address rewards either all commit or all roll back. A tick error stops the service rather than silently skipping accounting.

The connection holds an exclusive file lock: only one service may own a database, preventing accidental double-speed minting from two processes. External SQLite readers are also blocked while it runs; use the HTTP API, or stop the service before inspecting/copying the database. Excess HTTP requests receive a retryable busy response; the internal clock waits for queue capacity instead of shutting down under load.

| Table | Stored data | Meaning |
|---|---|---|
| `accounts` | `id` (integer primary key), unique `address`, `reward_sparks` | One user per canonical receiving address, with materialized lifetime rewards. No private key or mnemonic. |
| `contributions` | `id`, unique `request_id`, `user_id` (foreign key to `accounts.id`), `budget_sats`, `duration`, `delay_blocks`, `start_block` | Immutable submission and burn schedule. Several contributions may belong to one user. |
| `blocks` | `height` (primary key), `burned_sats`, `fees_sats`, `reward_sparks` | Committed simulator clock and per-block totals, including empty blocks. |
| `accruals` | `(height, contribution_id)` (primary key), `burned_sats`, `fees_sats`, `reward_sparks` | Auditable per-block allocation to each active contribution. |

Relationships: an account has many contributions; a contribution and a block each have many accruals. Earned amounts and fees on the contribution table are derived from accruals, not maintained as another mutable copy. Status is derived from the current block and schedule. The account total is deliberately materialized to make later reward export straightforward; it equals the sum of that address's accruals. All money is stored as SQLite signed 64-bit integers, never floats; proportional multiplication uses 128-bit intermediates. Cumulative monetary values in JSON are decimal strings to avoid JavaScript's integer precision limit.

User IDs start at 1 and increment for each new canonical address at its first accepted contribution. Repeat submissions, retries and uppercase spellings reuse the same user ID. IDs persist across service restarts; they are distinct from contribution IDs. The snapshot includes both `user_id` and receiving `address` for every contribution. Existing integer-user-ID databases are migrated without resetting the ledger: old schedules, IDs, allocations, balances and retries remain intact, including when lifting the former slider-based SQL limits. Old schemas without delays get zero delays. Historical durations below the new minimum remain valid records; the older address-keyed schema is not supported.

The submission ID is generated in the browser. Repeating an ID with identical canonical address, budget, duration, and delay returns the original contribution and start block without creating another one. Reusing it with different parameters is rejected. Omitted delay means zero, including old saved requests. Matching retries of previously accepted legacy durations remain valid. Because submissions and block ticks share one queue, the delay is applied relative to the committed height observed by the first accepted submission, not by its retries.

A pending public request is saved locally before submission, keeping its ID and settings across network errors and page reloads. Retrying is safe. Explicitly discarding a pending request does not cancel an already-accepted contribution; the UI warns before doing so.

### HTTP API and future wallet integration

| Endpoint | Purpose |
|---|---|
| `GET /api/state` | Dashboard snapshot: height, server-computed lifetime totals/count, and contributions newest first, including scheduled future starts. Contributions whose last block is over 10,000 blocks old are omitted. |
| `GET /api/state?full_history=true` | Mint-page snapshot with complete contribution history, preserving personal lifetime totals. |
| `POST /api/contributions` | JSON `{address, budget_sats, duration, delay_blocks, request_id}`; omitted `delay_blocks` defaults to zero. Returns `{id, user_id, start_block}`. |
| `GET /api/rewards` | Address-sorted accumulated rewards: `[{"address":"t1…i1…","reward_sparks":"5000000000"}]`. |

The reward export represents **lifetime earned rewards**, not a spendable balance or an instruction to pay them again. Creating actual Flame UTXOs is deliberately out of scope. The wallet integration must add an atomic issuance/checkpoint ledger (or export an immutable snapshot at a selected height) to track already-issued rewards and avoid duplicate payouts. It can use the stored receiving address directly; it must not require a user's private keys.

The database retains all contributions and per-block accruals; the dashboard response filter does not delete or archive records. Pagination, archival, authentication, real Bitcoin/Flame connectivity, cancellation and withdrawals are intentionally absent. No external assets, analytics or third-party scripts are loaded by the UI; both pages make a public price-data request for the BTC/USD ticker.

The sat / BTC / USD switch changes display only and remembers the selection in this browser. FLM counts remain whole numbers; sat amounts and rates have no decimals, BTC has exactly eight, and USD has two (three in the top total, up to six for $/FLM rates). Top counters use short currency-free labels (`Burned`, `Earned`, and `Current rate` or `Effective rate`) above values with units: `$X.XXX`, `X btc`, or `X sat`; `X FLM`; and `X $/FLM`, `X BTC/FLM`, or `X sat/FLM`, using non-breaking spaces between numbers and units. Decimal fractions are truncated for display only. Calculations, stored satoshis/sparks, submissions, and API values retain their precision. Effective rates use the untruncated reward amount, not the displayed whole-FLM value.

Both pages always show the BTC/USD price in a reserved single-line ticker. Coinbase's public [BTC/USD spot endpoint](https://docs.cdp.coinbase.com/coinbase-app/track-apis/prices) is requested without credentials on page load and refreshed once a minute, regardless of selected currency. The last valid quote stays visible during refresh; failed or invalid quotes clear it and leave USD values unavailable. Switching to USD uses the preloaded quote without a new request. The current reference quote revalues all amounts, including historical totals; these are not historical USD costs or execution prices.

## User UI

URL: `/mint`

The top counters show only this user's accrued totals: sats burned, mining fees paid, FLM earned, and effective rate `sats burned / FLM earned`, excluding fees. They include all of the user's contributions, not only the chart window. Scheduled future spending and the unsubmitted draft are excluded. Effective rate is `—` until rewards are earned; all numeric values are `—` while the wallet is unavailable, with their labels and units retained.

User sees a chart of the burn rate in satoshis per block, with the equivalent mint rate (sat/FLM) on the right-hand axis.
- Chart is centered around the current block, initially ±432 blocks, with its range controlled by zoom and labels shown as block numbers or calendar dates.
- Other users are grey. Everyone's known schedules are drawn exactly, including future starts and expirations.
- The user's submitted contributions are drawn on top using their assigned user-ID color, matching the dashboard.
- The user's draft contribution is a dashed line in the same color; it changes with the sliders. Both legend swatches match. Before the first accepted contribution assigns a user ID, the draft and swatches are neutral grey rather than guessing a future ID.

Controls:
- slider "budget": 100,000 to 100,000,000 satoshis (linear scale, in steps of 100,000).
- slider "duration": 144 to 4,464 blocks (1 to 31 days), in steps of one block.
- slider "delay": 0 to 4,464 blocks (0 to 31 days), in steps of one block; default zero.
- Adjacent text fields accept larger values than the sliders, in the selected currency/time units.
- display: projected total amount of flames to be earned over the selected delayed window, total amount of satoshis spent on fees, burned sats, and resulting average mint rate (burned sats/FLM), excluding fees.
- button "submit".

For a draft starting at `H + 1 + delay_blocks`, each active block `b` estimates `50 FLM × draft_burn(b) / (sum(existing_burn(b)) + draft_burn(b))`. All existing contributions, including this user's overlapping contributions and other users' scheduled starts/expirations, follow their actual schedules throughout the full duration, even outside the chart window. The estimator sums intervals between burn-rate changes instead of iterating over every block, so very long durations do not stall the page. Only the draft's share is included in its estimate. Later submissions can change the eventual result. Dates beyond the calendar's representable range display a block number instead.

Table below shows all user's contributions: 
- their status (scheduled, active, finished),
- amount of sats burned, 
- amount of FLM earned,
- total mining fees paid,
- average burned sats/FLM rate, excluding fees, per contribution.

Total line shows total sats burned, total FLM earned, total mining fees paid, and average burned sats/FLM, excluding fees. The dashboard table uses the same rate definition.

The reward address and recovery controls appear below the contributions table. Both pages end with a row of small squares showing the participant palette in its assigned order.

Blocks are issued every 1 second. Only blocks with active minters distribute 50 FLM; empty-block rewards are permanently lost.

On top - add a button that opens dashboard (`/` url).

## Dashboard

URL: `/`

Similar to per-user UI, but visible to everyone. The chart uses the selected symmetric zoom around the current block, including everyone's known future commitments. Contribution summaries show their budgets, durations, and schedules, including future starts. The API and table exclude only contributions with `height - (start_block + duration - 1) > 10,000`; the exact 10,000-block boundary remains included. The table is newest-first. Calendar mode uses the same selected block window.

The top counters and table footer use server-computed lifetime totals, including contributions omitted from the response. The footer's contribution count is also all-time, not just the number of visible rows.

The dashboard chart, "Burn rate and mining fees", shows historical and scheduled burned sats above zero and mining-fee income downward: 1,000 sat per active contribution per block, including overlapping contributions to the same address. Fees use a grey diagonal fill. Both areas share one scale in sat/block on the left; the right-hand scale is the same values divided by 50 FLM (sat/FLM), including negative ticks below zero. The downward direction separates fees from burning, not a loss of income. Both charts mark the current block and distinguish their future portion; `/mint` uses the same axis units without the fee area.

The dashboard always shows contributions colored by user, with no color toggle. The six original colors are retained, with one rounded RGB midpoint between each consecutive pair (including the last/first pair), giving twelve colors. Their fixed order maximizes the weakest adjacent perceptual contrast in OKLab, including the repeating last/first boundary; total adjacent distance breaks ties. The computed order is stored in `PALETTE` in `web/app.js`, not generated at runtime. All clients use `palette[user_id % palette.length]`; colors repeat every twelve users without randomization or browser-local assignment. Participant colors are opaque; mining fees remain grey and hatched.

The dashboard combines each user's contributions into one band. Bands are stacked by each user's latest contribution ID, oldest at the bottom and newest at the top. A new contribution moves that user's entire band upward without changing their color. The table remains newest-first.

On very top show four totals/rates:
- total satoshis burned from the beginning,
- total mining fees paid from the beginning,
- and total flames distributed,
- current mint rate: this block's burned sats divided by 50 FLM, excluding fees; `—` when no contributions burn this block. This is not the lifetime average.

And add a button that leads to `/mint` page.
