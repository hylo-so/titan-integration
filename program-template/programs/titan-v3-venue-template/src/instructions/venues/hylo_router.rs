use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use hylo_idl::pda;
use hylo_idl::router::client::args::RouteV2;
use hylo_idl::router::instruction_builders::route_v2;

use crate::error::TemplateError;

/// Builds the `hylo-router` `route_v2` instruction for one route leg.
pub fn swap(
  token_a: Pubkey,
  token_b: Pubkey,
  amount_in: u64,
  account_metas: &[AccountMeta],
) -> Result<Vec<Instruction>> {
  let args = RouteV2 {
    token_a,
    token_b,
    amount: amount_in,
    slippage_config: None,
  };
  let inner_accounts = account_metas
    .split_first()
    .filter(|(account, _)| account.pubkey == pda::EXO_REGISTRY)
    .map(|(_, accounts)| accounts)
    .ok_or(error!(TemplateError::InvalidSwapInput))?;
  Ok(vec![route_v2(&args, &inner_accounts.to_vec())])
}
