# Titan Integration — Hylo V2

Hylo's V2 integration for Titan's routing layer, built on Titan's AMM
integration template.

## Hylo V2 venue

Hylo is a collateralized exchange, not a pool AMM: it mints and redeems the
hyUSD stablecoin and a levercoin against collateral, converts between them,
swaps LST<->LST through its vaults, and swaps collateral<->USDC through the
USDC pair. One venue (`src/hylo/`, `HyloRouter`) covers every direction.

Two kinds of pairs:

- Enshrined pairs compiled into the exchange and earn pool programs: the ten
  bidirectional pairs in `ENSHRINED_PAIRS` over
  `[jitoSOL, hyloSOL, hyUSD, xSOL, sHYUSD, USDC]`.
- Exo pairs listed in the router's `ExoRegistry` (`pda::EXO_REGISTRY`). Each
  entry adds collateral<->hyUSD, collateral<->levercoin, collateral<->USDC and
  levercoin<->hyUSD. `update_state` reads the registry on every refresh and
  refetches once when the entry list changed, so a newly registered pair
  becomes quotable without a restart.

Both halves go through Hylo's standard interfaces, exactly like the Jupiter
integration: quotes call `ProtocolState::runtime_quote` from `hylo-quotes`
(the same `hylo-core` math the on-chain program executes), bounds come from
`runtime_min_input` / `runtime_max_input`, and every swap leg is one
`hylo-router` `route_v2` instruction. The router resolves the exchange
instruction from the mint pair on-chain.

Layers:

- Quote: `src/hylo/` (`HyloRouter`, `quotes::runtime_quote`,
  `instructions::swap_instruction`, `parse_pool_creations`). A "pool
  creation" is the static `pda::HYLO` pool plus one `pda::EXO_REGISTRY` pool
  per router `register_exo_entry` instruction.
- Route builder: `Venue::Hylo { token_a, token_b }` in
  `src/swap_route/mod.rs`
- Program: `program-template/.../instructions/venues/hylo_router.rs`
- Tests: `tests/hylo.rs`, `tests/hylo_creation.rs`,
  `program-template/.../tests/hylo_route.rs`

### Deployment

Both crates target the canonical mainnet programs
(`HYEXCHtHkBagdStcJCp3xbbb9B7sdMdWXFNj6mdsG4hn`,
`hyRouTRDAgn65xyyJ3L5c4k5SFmSdr3NxDV8Euzjy3f`,
`HysTabVUfmQBFcmzu1ctRd1Y1fxd66RBpboy1bmtDSQQ`). The `shadow` cargo feature
is off by default and points every program id and PDA at the shadow
deployment; `HYLO_EXCHANGE` / `HYLO_ROUTER` in the `Makefile` would need the
same switch.

### Program dumps

The simulation tests load dumped program binaries from two directories:
`programs/` for the venue suite and `program-template/program-dumps/` for the
route program test. Both dump only when a file is missing, so after an
on-chain upgrade delete the stale `.so` files and run `make dump-programs`
(the route test redumps on its own).

### Known caveats

`program-template/.../tests/hylo_route.rs` resyncs the LiteSVM clock from
RPC before every simulated swap while every other account stays cached from
the first fetch. Once wall time since that fetch exceeds the Pyth
`oracle_interval_secs`, the exchange rejects the cached feed with
`PythOracleOutdated` and the test reads red. With 68 directions and ten
samples each the run exceeds the window, so the test currently fails until
the harness takes one consistent snapshot.

---

# Titan AMM Integration Template

A reference implementation and test suite for adding AMMs, CLMMs, and proprietary liquidity engines to Titan’s routing layer.

## Overview

Titan aggregates liquidity from heterogeneous venues (AMMs, CLMMs, orderbooks, proprietary pools) under a single unified quoting and routing interface.

This repository provides:

- A compact `TradingVenue` template for describing quote math, token metadata, account loading, and swap instruction shape
- A robust boundary-search engine for computing safe swap-size ranges
- Token metadata utilities, including Token-2022 support
- A caching abstraction for efficient on-chain account loading
- Simulation tests using LiteSVM ensuring off-chain quotes match on-chain execution
- Pricing tests ensuring the reported marginal price is consistent with the quoted output

## On-Chain CPI Template

This repo also includes `program-template/`, an Anchor template for the venue CPI
adapter Titan's router program calls during routed swaps.

Use it to verify the venue's on-chain swap instruction shape against Titan
router account layout and TitanPDA custody:

```bash
cargo check --manifest-path program-template/Cargo.toml
make build-program
```

## Core Components

- `TradingVenue`: implement account parsing, state refresh, token metadata,
  protocol labeling, exact-in quote math, and swap instruction construction.
- `QuoteRequest` / `QuoteResult`: Titan routes `ExactIn` only. All amounts and
  prices use raw atom units, not UI decimal scaling.
- `QuoteResult::price`: report the marginal derivative
  `d(output_atoms) / d(input_atoms)`. It must be positive, non-increasing, and
  consistent with `expected_output`.
- `bounds`: finds safe input ranges from a zero-input-safe `quote()`.
- `TokenInfo`: covers SPL Token, Token-2022, and transfer fee metadata. Do not
  duplicate transfer-fee handling in quote math.
- `AccountsCache`: loads required on-chain accounts with RPC caching.

## Included Tests

The venue must pass the shared suite in `tests/common/mod.rs`, run through
`tests/hylo.rs`.

- Construction and boundaries: deserialization, state loading, token info,
  boundary quotes, and no heap allocation inside `quote()`.
- Simulation: LiteSVM swaps compare on-chain output to off-chain `quote()` at
  boundaries and random samples, while checking accounts, monotonicity, and
  quote speed.
- Pricing: `price` must be positive, non-increasing, and bracket the realized
  average rate: `price(b) <= (f(b) - f(a)) / (b - a) <= price(a)`. The tests
  include atom-rounding slack for truncated integer outputs.

## Running the tests

```bash
make build-program   # build the Titan router program template
make check-structure # fast no-RPC sanity checks
make test-venue     # the Hylo venue suite
make scorecard      # print the integration scorecard only
make dump-programs  # fetch the program binaries the simulation tests load
```

Everything runs cleanly on a fresh clone: the construction, simulation, and
pricing tests need a mainnet RPC endpoint (and, for the simulations, dumped
program binaries), so they **SKIP with an explanation** instead of failing when
those prerequisites are absent. To run them for real:

```bash
export SOLANA_RPC_URL=https://...   # a mainnet RPC endpoint
make build-program                  # rebuild the Titan router program template
make dump-programs                  # one-time: dump the venue programs into programs/
make test-venue
```

`make check-structure` runs unit tests, scorecard assertions, and `Venue` enum
parity checks without requiring RPC.

## Tips for Integrators
1. Always support zero-input quoting
2. Keep your deserialization strictly defensive, never panic
3. Don’t perform I/O, allocate heap memory, or panic inside quote()
4. A quote must average under 1 microsecond (1µs) — see the quoting_speed test
5. Make sure your instruction accounts match the program’s expectations
6. Report a marginal `price` in raw output atoms per raw input atom — positive, non-increasing in size, and consistent with `expected_output`
