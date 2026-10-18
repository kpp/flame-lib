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

Once browser-wallet assets have been built, `cargo run --locked` starts just the service. This is a standalone crate with its own lockfile; it does not change the protocol workspace's dependencies.

```sh
cargo test --locked
node --test web/test.mjs
```

## How it works

User chooses: 
- budget: amount of satoshis to spend total,
- duration: how many blocks to spend it over.

Each contribution starts at the next block after submission. Its total budget includes a fee of 1,000 sat **per contribution, per block**. The remaining sats are burned across the selected duration: each block burns the integer quotient, and the first remainder blocks burn one extra sat. Thus the complete budget is spent exactly, without fractional sats.

The budget slider runs linearly from 100,000 to 100,000,000 sat, in steps of 100,000. Submission requires `budget >= duration × 1,001`: enough for all fees and at least one burned sat per block. Invalid combinations are explained and cannot be submitted.

Blocks advance once per second while the service is running. Restarting resumes at the last committed height; downtime does not produce blocks or retroactive rewards. A funded block distributes exactly 50 FLM in proportion to each contribution's burned sats, excluding fees. Empty blocks distribute nothing. One FLM is 100,000,000 sparks. Allocation uses integer arithmetic: floor each exact share, then assign leftover sparks to largest fractional remainders, breaking ties by contribution ID. Forecasts hold other users' current burn rate constant and use the viewer's actual schedules; this assumption affects estimates only, not block processing.


The existing Flame wallet runs in the browser via WebAssembly and uses FlameKD to generate a mnemonic and testnet receiving address (`tf1…`). These are remembered in browser local storage, so subsequent contributions share the same reward destination. **The mnemonic, seed and private keys never go to the service or database.** Request parsing rejects extra fields, including mnemonic/private-key fields. Only the address and contribution parameters are submitted. The server validates the address with FlameKD and stores its canonical lowercase encoding.

Local storage is only suitable for this demo. Never reuse this mnemonic for real funds; clearing browser data loses it, and scripts running on the same origin can read it. The server cannot recover it. Address identity is public attribution, not authentication: this simulation does not prove address ownership, debit a real balance, or prevent someone submitting simulated contributions to another address. Keep the default loopback binding unless intentionally sharing the demo. A real paid deployment needs authentication, payment verification and abuse controls before accepting contributions.

## Data model

One dedicated thread owns the SQLite connection. HTTP requests and block ticks go through its bounded command queue; it never shares a connection behind a mutex or blocks an async worker with SQL. SQLite foreign keys, strict integer columns and checked arithmetic protect accounting. Block processing is one transaction: block record, contribution accruals and accumulated address rewards either all commit or all roll back. A tick error stops the service rather than silently skipping accounting.

The connection holds an exclusive file lock: only one service may own a database, preventing accidental double-speed minting from two processes. External SQLite readers are also blocked while it runs; use the HTTP API, or stop the service before inspecting/copying the database. Excess HTTP requests receive a retryable busy response; the internal clock waits for queue capacity instead of shutting down under load.

| Table | Stored data | Meaning |
|---|---|---|
| `accounts` | `id` (integer primary key), unique `address`, `reward_sparks` | One user per canonical receiving address, with materialized lifetime rewards. No private key or mnemonic. |
| `contributions` | `id`, unique `request_id`, `user_id` (foreign key to `accounts.id`), `budget_sats`, `duration`, `start_block` | Immutable submission and burn schedule. Several contributions may belong to one user. |
| `blocks` | `height` (primary key), `burned_sats`, `fees_sats`, `reward_sparks` | Committed simulator clock and per-block totals, including empty blocks. |
| `accruals` | `(height, contribution_id)` (primary key), `burned_sats`, `fees_sats`, `reward_sparks` | Auditable per-block allocation to each active contribution. |

Relationships: an account has many contributions; a contribution and a block each have many accruals. Earned amounts and fees on the contribution table are derived from accruals, not maintained as another mutable copy. Status is derived from the current block and schedule. The account total is deliberately materialized to make later reward export straightforward; it equals the sum of that address's accruals. All money is stored as SQLite signed 64-bit integers, never floats; proportional multiplication uses 128-bit intermediates. Cumulative monetary values in JSON are decimal strings to avoid JavaScript's integer precision limit.

User IDs start at 1 and increment for each new canonical address at its first accepted contribution. Repeat submissions, retries and uppercase spellings reuse the same user ID. IDs persist across service restarts; they are distinct from contribution IDs. The snapshot includes both `user_id` and receiving `address` for every contribution. This schema starts from a fresh database; the previous address-keyed schema is not migrated automatically.

The submission ID is generated in the browser. Repeating an ID with identical canonical address, budget and duration returns the original contribution without creating another one. Reusing it with different parameters is rejected. Because submissions and block ticks share one queue, a submission at a block boundary always starts at the next block relative to the committed height it observes.

A pending public request is saved locally before submission, keeping its ID and settings across network errors and page reloads. Retrying is safe. Explicitly discarding a pending request does not cancel an already-accepted contribution; the UI warns before doing so.

### HTTP API and future wallet integration

| Endpoint | Purpose |
|---|---|
| `GET /api/state` | Dashboard snapshot: height, server-computed lifetime totals/count, and started contributions, newest first. Not-yet-started contributions and those whose last block is over 10,000 blocks old are omitted. |
| `GET /api/state?full_history=true` | Mint-page snapshot with complete contribution history, preserving personal lifetime totals. |
| `POST /api/contributions` | JSON `{address, budget_sats, duration, request_id}`; returns `{id, user_id, start_block}`. |
| `GET /api/rewards` | Address-sorted accumulated rewards: `[{"address":"tf1…","reward_sparks":"5000000000"}]`. |

The reward export represents **lifetime earned rewards**, not a spendable balance or an instruction to pay them again. Creating actual Flame UTXOs is deliberately out of scope. The wallet integration must add an atomic issuance/checkpoint ledger (or export an immutable snapshot at a selected height) to track already-issued rewards and avoid duplicate payouts. It can use the stored receiving address directly; it must not require a user's private keys.

The database retains all contributions and per-block accruals; the dashboard response filter does not delete or archive records. Pagination, archival, authentication, real Bitcoin/Flame connectivity, cancellation and withdrawals are intentionally absent. No external assets, analytics or third-party scripts are loaded by the UI.

FLM amounts and sat/FLM rates are displayed as whole numbers with decimal fractions truncated, including projections. Calculations, stored sparks, and API values retain their precision. Effective rates use the untruncated reward amount, not the displayed whole-FLM value.

## User UI

URL: `/mint`

The top counters show only this user's accrued totals: sats burned, FLM earned, and effective cost `(sats burned + mining fees paid) / FLM earned`. They include all of the user's contributions, not only the chart window. Scheduled future spending and the unsubmitted draft are excluded. Effective cost is `—` until rewards are earned; all three counters are `—` while the wallet is unavailable.

User sees a chart of the burn rate in satoshis per block, with the equivalent mint rate (sat/FLM) on the right-hand axis.
- Chart is centered around the current block and shows the previous 500 blocks and next 500 blocks. The past shows actual schedules; the future is explicitly a forecast.
- Other users are grey. Their combined burn rate at the current processed block is extended unchanged through the future, ignoring their scheduled expirations and not-yet-started contributions.
- The user's submitted contributions are drawn on top using their assigned user-ID color, matching the dashboard.
- The user's draft contribution is a dashed line in the same color; it changes with the sliders. Both legend swatches match. Before the first accepted contribution assigns a user ID, the draft and swatches are neutral grey rather than guessing a future ID.

Controls:
- slider "budget": 100,000 to 100,000,000 satoshis (linear scale, in steps of 100,000).
- slider "duration": slider from 100 to 1000 blocks (linear scale, in steps of 50 blocks).
- display: projected total amount of flames to be earned (based on currently running contributions), total amount of satoshis spent on fees, and resulting average mint rate (sats/FLM) that takes into account fees paid (that raise the price).
- button "submit".

For a draft starting after block `H`, let `O` be everyone else's combined burn at `H`. At each future block `b`, the draft's estimated reward is `50 FLM × draft_burn(b) / (O + existing_own_burn(b) + draft_burn(b))`. Existing contributions by this user retain their real start/end blocks, so overlapping contributions compete for the same issuance. Only the draft's share is included in its estimate. This flat-other-user assumption applies throughout the selected duration, including beyond the visible 500-block forecast; it never takes a scheduled drop in other users' contributions as a predicted price decrease. Actual future participation may differ.

This is a presentation and forecasting convention, not commitment secrecy: the existing API still carries schedules for reconstructing history, but the forecast never uses another user's future schedule.

Table below shows all user's contributions: 
- their status (active, finished),
- amount of sats burned, 
- amount of FLM earned,
- total mining fees paid,
- average sats/FLM rate - per each contribution.

Total line shows total sats burned, total FLM earned, total mining fees paid, average mint rate.

Blocks are issued every 1 second. Rewards are distributed at rate of 50 FLM per block, proportional to each active contribution.

On top - add a button that opens dashboard (`/` url).

## Dashboard

URL: `/`

Similar to per-user UI, but visible to everyone. The chart shows the previous 1,000 blocks, with the current block at its right edge and no future commitments. Public contribution summaries omit planned budgets and durations. The API and table exclude not-yet-started contributions and those with `height - (start_block + duration - 1) > 10,000`; the exact 10,000-block boundary remains included. The table is newest-first.

The top counters and table footer use server-computed lifetime totals, including contributions omitted from the response. The footer's contribution count is also all-time, not just the number of visible rows.

The dashboard chart, "Burn rate and mining fees", shows historical burned sats above zero and mining-fee income downward: 1,000 sat per active contribution per block, including overlapping contributions to the same address. Fees use a grey diagonal fill. Both areas share one scale in sat/block on the left; the right-hand scale is the same values divided by 50 FLM (sat/FLM), including negative ticks below zero. The downward direction separates fees from burning, not a loss of income. The `/mint` chart uses the same two axis units without the fee area and shades its future forecast.

The dashboard always shows contributions colored by user, with no color toggle. All clients use the fixed ordered palette `#ffd800`, `#ffc200`, `#ff9c00`, `#ff6922`, `#ff4242`, `#fff000`, with `palette[user_id % palette.length]`. Thus user 1 uses `#ffc200`, user 6 uses `#ffd800`, and colors repeat every six users. No randomization or browser-local palette is used. Participant colors are opaque; mining fees remain grey and hatched.

The dashboard combines each user's contributions into one band. Bands are stacked by each user's latest contribution ID, oldest at the bottom and newest at the top. A new contribution moves that user's entire band upward without changing their color. The table remains newest-first.

On very top show three big digits: 
- total satoshis burned from the beginning,
- and total flames distributed,
- current mint rate: this block's burned sats divided by 50 FLM, excluding fees; `—` when no contributions burn this block. This is not the lifetime average.

And add a button that leads to `/mint` page.
