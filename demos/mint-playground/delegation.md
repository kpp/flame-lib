# Delegated minting

Delegating users share a service's minting activity instead of paying their own mining fees. The service receives a percentage of their FLM allocation. This is a simulation; the accounting and HTTP API are described in the [README](README.md).

## Modes and service activity

Each contribution chooses Independent, Service or Delegating. Independent and Service reserve 1,000 sat per active contribution/block for mining fees and burn the remaining budget. Delegating burns its whole budget, pays no mining fees and identifies a provider by user ID.

A service offer belongs to the provider's identity and persists independently of contributions. It is active only while that user has an active Service-mode contribution. Delegations may start or end outside those windows: their coins still burn while the provider is inactive, but they earn no FLM or commission then. Inactive delegated burns are excluded from the reward denominator. There is no automatic fallback, refund or later recovery of those lost rewards.

## One current commission per identity

The provider's identity is the only place that defines its commission, initially 2%. Neither Service contributions nor Delegating contributions contain their own rates. Modes and provider links remain immutable; the identity rate is mutable.

Saving a rate applies it to **every past and future allocation for every delegator**, including finished contributions. Each historical block's gross allocation is unchanged. Its commission is recalculated as `floor(gross sparks × current basis points / 10,000)`, and the owner keeps the rest. Historical burns, mining fees and total FLM issuance never change. Self-delegation transfers commission back to the same account.

Rate changes update derived commissions and all affected account balances atomically. A new Service contribution initializes the default only if no rate exists; it never overwrites a saved rate. Contribution submission, old commission quotes and stale retries cannot change the identity's rate.

## Mint UI

- Use three radio buttons: Independent, Service, Delegating.
- Service shows a percentage field and explicit Save button. Save updates the identity, independently of submitting a contribution; changing the field alone does nothing.
- Delegating selects a service by name, current percentage and activity state. Its estimate uses the current saved rate, zero mining fees and the entire burned budget. Pending requests do not freeze rates.
- Estimates show FLM, fees, burn and effective burned-sats/FLM. Service estimates exclude hypothetical commission income; actual received commissions are included in its top totals. Self-delegation retains its gross share.
- All delegations are blue on charts; Independent/Service use the warm palette. Providers can inspect linked delegations in their personal tables.
- An own delegation row shows the owner's net reward. A provider's linked row shows commission income only, with zero provider burn/fees. Personal totals include own net rewards plus incoming commissions, without claiming delegates' spending.
- The first column shows `User name #N`, then budget / duration. A separate Minter column names the provider for delegations, otherwise the owner.
- Status shows `Scheduled at …`, `Finished on …`, or `Active X%`, using the selected date/block display. Progress measures the proportion of burn consumed.

Names, net rewards, percentages and effective rates update from current server state. Past net earnings can increase or decrease after a commission save.

Blue palette, indexed by user ID in the same way as the warm palette:

```text
#64b5ff #2354d8 #409bff #173a9e
#2478ed #83c7ff #3865ba #a3d9ff
#3289f5 #2b46ad #51a8ff #2d68db
```

## Dashboard

Use the same contribution identities, modes, colors and statuses. A sortable minter table shows name, own total burn, total reward including received commissions, mining fees and effective burned-sats/FLM. Put the current service percentage beside its name, not in another column. Latest contribution mode determines the account's presentation; it does not restrict mixed-mode history or create another commission setting.

## Demo limitation

Profiles are not authenticated. Anyone knowing a public address can change its service commission and redistribute simulated historical balances. Production would require authenticated rate changes and payout-finalization rules: already-issued real assets cannot be reassigned by this retrospective demo setting.

