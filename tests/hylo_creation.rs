use anchor_lang::prelude::AccountMeta;
use hylo_idl::pda;
use hylo_idl::router::client::args::RouteV2;
use hylo_idl::router::instruction_builders::{register_exo_entry, route_v2};
use hylo_idl::tokens::{CBBTC, HYUSD, TokenMint, USDC, XBTC};
use hylo_titan_integration::hylo::{enshrined_mints, parse_pool_creations};
use hylo_titan_integration::trading_venue::protocol::PoolProtocol;
use hylo_titan_integration::trading_venue::venue_creation::{
  ParsedInstruction, PoolCreation,
};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

fn base_pool() -> PoolCreation {
  PoolCreation {
    protocol: PoolProtocol::Hylo,
    pool: pda::EXO_REGISTRY,
    mints: enshrined_mints(),
  }
}

fn parsed(instruction: Instruction) -> ParsedInstruction {
  ParsedInstruction {
    program_id: instruction.program_id,
    accounts: instruction
      .accounts
      .iter()
      .map(|meta| meta.pubkey)
      .collect(),
    data: instruction.data,
  }
}

/// Hylo listings are admin-gated, so the base pool is static: the global
/// state account over every enshrined mint, regardless of input.
#[test]
fn static_pool_list() {
  assert_eq!(parse_pool_creations(&[]), vec![base_pool()]);
}

#[test]
fn registry_pool_per_registration() {
  let admin = Pubkey::new_unique();
  let registration = parsed(register_exo_entry(admin, CBBTC::MINT));
  let expected = PoolCreation {
    protocol: PoolProtocol::Hylo,
    pool: pda::EXO_REGISTRY,
    mints: vec![HYUSD::MINT, USDC::MINT, CBBTC::MINT, XBTC::MINT],
  };
  assert_eq!(
    parse_pool_creations(&[registration]),
    vec![base_pool(), expected]
  );
}

#[test]
fn other_router_instructions_add_nothing() {
  let args = RouteV2 {
    token_a: CBBTC::MINT,
    token_b: HYUSD::MINT,
    amount: 1,
    slippage_config: None,
  };
  let route = parsed(route_v2(&args, &Vec::<AccountMeta>::new()));
  assert_eq!(parse_pool_creations(&[route]), vec![base_pool()]);
}
