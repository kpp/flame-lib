# Mint strategy demo

Open [index.html](index.html) in a browser. The demo is self-contained and works
offline, with no build step or Flame dependencies. Run its checks with
`node --test demos/mint-strategy/test.mjs` from the repository root.

A user can join minting by choosing the budget (in satoshis) and how long they mint (in blocks). For simplicity, we split the budget in equal parts among all blocks.

The market is simulated as a total mint rate (burned satoshis per FLM per block) of all other that progresses from block to block:

1. User chooses at which block to start, the budget and the number of blocks.
2. Flames are distributed at each block (50 FLM per block) in proportion to the user's share relative to the total satoshis burned at the block.
3. In the end of the user's budget, total amount of flames is displayed, and an average mint rate: satoshis per FLM.
4. At each block there is a known per-block mint rate: how many satoshis per 1 FLM are burned.

The demo displays the chart of mint rate over 10000 blocks. One line for the market trend without user's part and another line - cumulative with the user (show it only on the interval where user mints).

Parameters:
1. Type of trend: linear, quadratic, exponential or sqrt. 
2. Slope of the trend: single slider - how steep is the trend (parameter varies for each kind).
   Noise: 0–30% standard deviation around the smooth trend, using a seeded,
   mean-reverting Gaussian log-walk with approximately 100-block correlation.
   Its lognormal multiplier has mean 1 and the selected relative standard
   deviation; 30% is not a hard swing limit. The path is fixed across strategy
   changes, shared by both charts, and rates stay positive. Zero disables noise.
3. Budget: 1,000–10,000,000 satoshis, on a logarithmic slider.
4. Starting block: zero-based, from 0 through `10,000 - blocks_to_mint`.
   Increasing the duration clamps the start if needed so the entire budget fits.
5. Blocks to mint: 100–5,000, on a logarithmic slider.

Demo displays the average mint rate in response to various user's choices for a given trend. User's goal is to maximize the return for any given budget and a given situation.

A second chart plots FLM minted for every integer duration from 100 to 5,000
blocks, holding the budget, starting block and trend fixed. Each candidate splits
the full budget evenly; durations that exceed the remaining horizon are omitted.
Its duration axis is logarithmic, and a point marks the currently selected window.

