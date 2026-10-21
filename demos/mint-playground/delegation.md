# Delegated minting

An independent node burns coins and makes mint-proof transaction ("endorsement") every bitcoin block. This necessarily incurs mining fees at each block.

To save on mining fees nodes can "delegate" minting. In this case the minter pays the same mining fees making one endorsement per block, but multiple contributions share the rewards. In return, minter receives a flat percentage commission from participants, which helps recover the cost of mining fees or even make a profit.

Users can select one of three modes of operating:

1 - "Independent": does everything by itself, pays minting fees.
2 - "Service": same as independent, but allows selecting % of profit share (2% by default).
3 - "Delegating": delegates minting to one of selected "Services". Does not pay mining fees, but pays percentage profit share to the selected "Service" (percentage of FLM rewards).

## Minter UI

For each contribution, user can choose one of three modes: Independent, Service, Delegating as choice between 3 radio buttons.

Below, there are options and estimated profits.

- Independent: no options.
- Service: text field to select percentage (2% by default).
- Delegating: select among the list of minters - where their names and comission percentages displayed.

Estimates are: total FLM reward, mining fees, total burn, effective mint rate sat/FLM.

Delegating user sees the estimated reward after deducting comission, zero mining fees, and the effective mint rate is also after deducting comission (based on actual reward). The independent user and service see effective mint rate without including the mining fees.

"Service" cannot estimate the earnings from delegating users, but sees factual contributions linked to it, and that reflects in their mint rate on top.

The chart colors in light blue all the delegating contributions. On the chart only and in the contributions list Service sees also blue-colored contributions linked to it. The table shows earning according to commission (so it sums up to the actual rewards of the Service, not including portion that goes to the delegating users).

Delegating users in their table see the portion of the reward they get after deducting Service commission, so it sums up to their total reward.

Contributions table shows an extra column after first one with "Minter" name - which for delegating users is the name of the Service, not the user.

The first column is simplified: one line with "User name #N" where N is the number of the contribution, then "Total amount / duration".

Status column specifies the durations in detail: if scheduled, then show "Scheduled at ____" with date or block number. If finished, then "Finished on ____" with date or block number. And if active, then show percentage of completeness - "Active X%" updated in real time. 50% means half of the contribution is burned.

Light blue shades: ['#64b5ff', '#2354d8', '#409bff', '#173a9e',
 '#2478ed', '#83c7ff', '#3865ba', '#a3d9ff',
 '#3289f5', '#2b46ad', '#51a8ff', '#2d68db'].


## Dashboard UI

Same adjustments to colors and contributions table as in minter UI. 

Also show summary and kind of minters we have by name, as a table sortable by columns. Columns: name, total burn, total reward, total mining fees, mint rate. Use yellow colors for indepenednet / services, blueish for delegating users. For services show % comission near the name (not in a separate column).

