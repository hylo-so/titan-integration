use hylo_idl::router::types::ExoEntry;
use hylo_quotes::prelude::ProtocolState;
use hylo_quotes::token_operation::RuntimeQuote;
use solana_program::clock::Clock;
use solana_pubkey::Pubkey;

use super::error::exceeds_liquidity;
use crate::trading_venue::error::TradingVenueError;

/// Output amount and marginal rate of a [`RuntimeQuote`].
pub struct HyloRuntimeQuote {
  pub expected_output: u64,
  pub price: f64,
}

/// Output amount and marginal rate for a swap. `Ok(None)` when the size
/// exceeds available liquidity.
pub fn runtime_quote(
  state: &ProtocolState<Clock>,
  exo_entries: &[ExoEntry],
  input_mint: Pubkey,
  output_mint: Pubkey,
  amount: u64,
) -> Result<Option<HyloRuntimeQuote>, TradingVenueError> {
  match state.runtime_quote(exo_entries, input_mint, output_mint, amount) {
    Ok(RuntimeQuote {
      out_amount,
      marginal_rate,
      ..
    }) => Ok(Some(HyloRuntimeQuote {
      expected_output: out_amount,
      price: marginal_rate,
    })),
    Err(err) if exceeds_liquidity(err) => Ok(None),
    Err(err) => Err(err.into()),
  }
}
