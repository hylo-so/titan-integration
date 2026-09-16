use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use hylo_idl::pda;
use hylo_idl::router::client::args::Route;
use hylo_idl::router::instruction_builders::route;

/// Builds the `hylo-router` `route` instruction for one route leg.
pub fn swap(
  token_a: Pubkey,
  token_b: Pubkey,
  amount_in: u64,
  account_metas: &[AccountMeta],
) -> Result<Vec<Instruction>> {
  let args = Route {
    token_a,
    token_b,
    amount: amount_in,
    slippage_config: None,
  };
  let accounts =
    std::iter::once(AccountMeta::new_readonly(pda::EXO_REGISTRY, false))
      .chain(account_metas.iter().cloned())
      .collect::<Vec<_>>();
  Ok(vec![route(&args, &accounts)])
}
