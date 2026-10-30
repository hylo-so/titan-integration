use hylo_idl::router::types::ExoEntry;
use hylo_quotes::prelude::ProtocolState;
use hylo_quotes::token_operation::RuntimeQuote;
use solana_program::clock::Clock;

use super::error::exceeds_liquidity;
use crate::trading_venue::error::TradingVenueError;
use crate::trading_venue::{QuoteRequest, QuoteResult};

/// Exact-in quote. A size the protocol cannot fill yields a zero quote
/// flagged `not_enough_liquidity`.
pub fn quote(
  state: &ProtocolState<Clock>,
  exo_entries: &[ExoEntry],
  &QuoteRequest {
    input_mint,
    output_mint,
    amount,
    ..
  }: &QuoteRequest,
) -> Result<QuoteResult, TradingVenueError> {
  let unfilled = QuoteResult {
    input_mint,
    output_mint,
    amount,
    expected_output: 0,
    not_enough_liquidity: true,
    price: 0.0,
  };
  match state.runtime_quote(exo_entries, input_mint, output_mint, amount) {
    Ok(RuntimeQuote {
      out_amount,
      marginal_rate,
      ..
    }) => Ok(QuoteResult {
      expected_output: out_amount,
      not_enough_liquidity: false,
      price: marginal_rate,
      ..unfilled
    }),
    Err(err) if exceeds_liquidity(err) => Ok(unfilled),
    Err(err) => Err(err.into()),
  }
}
