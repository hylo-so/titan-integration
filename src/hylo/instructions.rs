use anchor_lang::ToAccountMetas;
use anchor_lang::prelude::AccountMeta;
use hylo_idl::earn_pool::account_builders::{deposit, withdraw};
use hylo_idl::exchange::account_builders::{
  convert_lever_to_stable_exo, convert_lever_to_stable_lst,
  convert_stable_to_lever_exo, convert_stable_to_lever_lst, mint_levercoin_exo,
  mint_levercoin_lst, mint_stablecoin_exo, mint_stablecoin_lst,
  mint_stablecoin_usdc, redeem_levercoin_exo, redeem_levercoin_lst,
  redeem_stablecoin_exo, redeem_stablecoin_lst, redeem_stablecoin_usdc,
  swap_exo_to_usdc, swap_lst_to_lst, swap_lst_to_usdc, swap_usdc_to_exo,
  swap_usdc_to_lst,
};
use hylo_idl::router::client::args::RouteV2;
use hylo_idl::router::instruction_builders::route_v2;
use hylo_idl::router::types::ExoEntry;
use hylo_idl::tokens::ExoRole::{Collateral, Levercoin, Stablecoin, Usdc};
use hylo_idl::tokens::{
  HYLOSOL, HYUSD, JITOSOL, SHYUSD, StakePool, TokenMint, USDC, XSOL,
};
use hylo_quotes::protocol_state::{
  exo_entry_role, exo_pyth_feed_by_mint, find_exo_entry,
};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use crate::trading_venue::QuoteRequest;
use crate::trading_venue::error::TradingVenueError;

/// Accounts for the exo route between `input_mint` and `output_mint`,
/// erroring when no registered pair owns either mint.
fn exo_accounts(
  user: Pubkey,
  exo_entries: &[ExoEntry],
  input_mint: Pubkey,
  output_mint: Pubkey,
) -> Result<Vec<AccountMeta>, TradingVenueError> {
  let entry = find_exo_entry(exo_entries, input_mint, output_mint)
    .ok_or(TradingVenueError::InvalidMint(input_mint.into()))?;
  let collateral = entry.collateral_mint;
  let oracle = exo_pyth_feed_by_mint(collateral)?.address;
  let roles = (
    exo_entry_role(entry, input_mint)?,
    exo_entry_role(entry, output_mint)?,
  );
  let accounts = match roles {
    (Collateral, Stablecoin) => {
      mint_stablecoin_exo(user, collateral, oracle).to_account_metas(None)
    }
    (Stablecoin, Collateral) => {
      redeem_stablecoin_exo(user, collateral, oracle).to_account_metas(None)
    }
    (Collateral, Levercoin) => {
      mint_levercoin_exo(user, collateral, oracle).to_account_metas(None)
    }
    (Levercoin, Collateral) => {
      redeem_levercoin_exo(user, collateral, oracle).to_account_metas(None)
    }
    (Stablecoin, Levercoin) => {
      convert_stable_to_lever_exo(user, collateral, oracle)
        .to_account_metas(None)
    }
    (Levercoin, Stablecoin) => {
      convert_lever_to_stable_exo(user, collateral, oracle)
        .to_account_metas(None)
    }
    (Collateral, Usdc) => {
      swap_exo_to_usdc(user, collateral, oracle).to_account_metas(None)
    }
    (Usdc, Collateral) => {
      swap_usdc_to_exo(user, collateral, oracle).to_account_metas(None)
    }
    _ => Err(TradingVenueError::InvalidMint(input_mint.into()))?,
  };
  Ok(accounts)
}

/// Builds the `hylo-router` `route_v2` instruction for a swap direction,
/// erroring on an unroutable pair. One arm per enshrined pair, mirroring
/// `hylo-router`'s `resolve_route`; registered exo pairs fall through to
/// [`exo_accounts`].
pub fn swap_instruction(
  &QuoteRequest {
    input_mint,
    output_mint,
    amount,
    ..
  }: &QuoteRequest,
  user: Pubkey,
  exo_entries: &[ExoEntry],
) -> Result<Instruction, TradingVenueError> {
  let accounts = match (input_mint, output_mint) {
    (lst @ (JITOSOL::MINT | HYLOSOL::MINT), HYUSD::MINT) => {
      mint_stablecoin_lst(user, lst).to_account_metas(None)
    }
    (lst @ (JITOSOL::MINT | HYLOSOL::MINT), XSOL::MINT) => {
      mint_levercoin_lst(user, lst).to_account_metas(None)
    }
    (HYUSD::MINT, lst @ (JITOSOL::MINT | HYLOSOL::MINT)) => {
      redeem_stablecoin_lst(user, lst).to_account_metas(None)
    }
    (XSOL::MINT, lst @ (JITOSOL::MINT | HYLOSOL::MINT)) => {
      redeem_levercoin_lst(user, lst).to_account_metas(None)
    }
    (HYUSD::MINT, XSOL::MINT) => {
      convert_stable_to_lever_lst(user).to_account_metas(None)
    }
    (XSOL::MINT, HYUSD::MINT) => {
      convert_lever_to_stable_lst(user).to_account_metas(None)
    }
    (JITOSOL::MINT, HYLOSOL::MINT) => {
      swap_lst_to_lst(user, JITOSOL::MINT, HYLOSOL::MINT).to_account_metas(None)
    }
    (HYLOSOL::MINT, JITOSOL::MINT) => {
      swap_lst_to_lst(user, HYLOSOL::MINT, JITOSOL::MINT).to_account_metas(None)
    }
    (JITOSOL::MINT, USDC::MINT) => {
      swap_lst_to_usdc(user, JITOSOL::MINT, JITOSOL::POOL_STATE)
        .to_account_metas(None)
    }
    (HYLOSOL::MINT, USDC::MINT) => {
      swap_lst_to_usdc(user, HYLOSOL::MINT, HYLOSOL::POOL_STATE)
        .to_account_metas(None)
    }
    (USDC::MINT, JITOSOL::MINT) => {
      swap_usdc_to_lst(user, JITOSOL::MINT, JITOSOL::POOL_STATE)
        .to_account_metas(None)
    }
    (USDC::MINT, HYLOSOL::MINT) => {
      swap_usdc_to_lst(user, HYLOSOL::MINT, HYLOSOL::POOL_STATE)
        .to_account_metas(None)
    }
    (USDC::MINT, HYUSD::MINT) => {
      mint_stablecoin_usdc(user).to_account_metas(None)
    }
    (HYUSD::MINT, USDC::MINT) => {
      redeem_stablecoin_usdc(user).to_account_metas(None)
    }
    (HYUSD::MINT, SHYUSD::MINT) => deposit(user).to_account_metas(None),
    (SHYUSD::MINT, HYUSD::MINT) => withdraw(user).to_account_metas(None),
    _ => exo_accounts(user, exo_entries, input_mint, output_mint)?,
  };
  let args = RouteV2 {
    token_a: input_mint,
    token_b: output_mint,
    amount,
    slippage_config: None,
  };
  Ok(route_v2(&args, &accounts))
}
