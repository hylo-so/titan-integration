mod error;
mod instructions;
mod quotes;

use std::iter::once;

use anchor_lang::AccountDeserialize;
use async_trait::async_trait;
use hylo_idl::exchange::accounts::Hylo;
use hylo_idl::router::instruction_builders::register_exo_entry;
use hylo_idl::router::types::ExoEntry;
use hylo_idl::tokens::{
  HYLOSOL, HYUSD, JITOSOL, SHYUSD, TokenMint, USDC, XSOL,
};
use hylo_idl::{earn_pool, exchange, pda, router};
use hylo_quotes::prelude::ProtocolState;
use hylo_quotes::protocol_state::{
  EXO_PAIR_ACCOUNTS, ProtocolAccounts, exo_pubkeys_from_entries,
  read_exo_entries,
};
use solana_account::Account;
use solana_instruction::Instruction;
use solana_program::clock::Clock;
use solana_pubkey::Pubkey;

use self::error::error_chain;
use self::quotes::HyloRuntimeQuote;
use crate::account_caching::AccountsCache;
use crate::trading_venue::error::TradingVenueError;
use crate::trading_venue::protocol::PoolProtocol;
use crate::trading_venue::token_info::{TOKEN_PROGRAM_ID, TokenInfo};
use crate::trading_venue::venue_creation::{ParsedInstruction, PoolCreation};
use crate::trading_venue::{
  FromAccount, QuoteRequest, QuoteResult, SwapType, TradingVenue,
};

/// Bidirectional pairs compiled into the exchange and earn pool programs.
/// Exogenous pairs come from the Hylo router's `exo_registry` at runtime.
pub const ENSHRINED_PAIRS: [[Pubkey; 2]; 10] = [
  [JITOSOL::MINT, HYUSD::MINT],
  [JITOSOL::MINT, XSOL::MINT],
  [JITOSOL::MINT, HYLOSOL::MINT],
  [JITOSOL::MINT, USDC::MINT],
  [HYLOSOL::MINT, HYUSD::MINT],
  [HYLOSOL::MINT, XSOL::MINT],
  [HYLOSOL::MINT, USDC::MINT],
  [USDC::MINT, HYUSD::MINT],
  [HYUSD::MINT, XSOL::MINT],
  [HYUSD::MINT, SHYUSD::MINT],
];

/// Unique mints appearing in [`ENSHRINED_PAIRS`].
#[must_use]
pub fn enshrined_mints() -> Vec<Pubkey> {
  ENSHRINED_PAIRS
    .as_flattened()
    .iter()
    .fold(Vec::new(), |mut acc, mint| {
      if !acc.contains(mint) {
        acc.push(*mint);
      }
      acc
    })
}

/// Collateral mint of a router `register_exo_entry` instruction. Rebuilds
/// the instruction from each account through the SDK builder and matches
/// program id, data and the whole account list.
fn registered_collateral(instruction: &ParsedInstruction) -> Option<Pubkey> {
  let admin = *instruction.accounts.first()?;
  instruction.accounts.iter().copied().find(|&candidate| {
    let expected = register_exo_entry(admin, candidate);
    expected.program_id == instruction.program_id
      && expected.data == instruction.data
      && expected
        .accounts
        .iter()
        .map(|meta| meta.pubkey)
        .eq(instruction.accounts.iter().copied())
  })
}

/// Registry pool for a router `register_exo_entry` instruction.
fn exo_registration(instruction: &ParsedInstruction) -> Option<PoolCreation> {
  let collateral_mint = registered_collateral(instruction)?;
  Some(PoolCreation {
    protocol: PoolProtocol::Hylo,
    pool: pda::EXO_REGISTRY,
    mints: vec![
      HYUSD::MINT,
      USDC::MINT,
      collateral_mint,
      pda::exo_levercoin_mint(collateral_mint),
    ],
  })
}

/// Base pool over the enshrined mints, then one registry pool per
/// `register_exo_entry` instruction.
#[must_use]
pub fn parse_pool_creations(
  instructions: &[ParsedInstruction],
) -> Vec<PoolCreation> {
  let base = PoolCreation {
    protocol: PoolProtocol::Hylo,
    pool: pda::HYLO,
    mints: enshrined_mints(),
  };
  once(base)
    .chain(instructions.iter().filter_map(exo_registration))
    .collect()
}

/// External mint accounts fetched alongside [`ProtocolAccounts`].
struct ExternalMints<'a> {
  jitosol: &'a Account,
  hylosol: &'a Account,
  usdc: &'a Account,
}

impl<'a> ExternalMints<'a> {
  const PUBKEYS: [Pubkey; 3] = [JITOSOL::MINT, HYLOSOL::MINT, USDC::MINT];

  /// Borrows a fetched account list, erroring with the key of the first
  /// missing account.
  fn from_fetched(
    fetched: &'a [Option<Account>],
  ) -> Result<Self, TradingVenueError> {
    if let [Some(jitosol), Some(hylosol), Some(usdc)] = fetched {
      Ok(ExternalMints {
        jitosol,
        hylosol,
        usdc,
      })
    } else {
      let (key, _) = Self::PUBKEYS
        .iter()
        .zip(fetched)
        .find(|(_, account)| account.is_none())
        .ok_or(TradingVenueError::FailedToFetchMultipleAccountData)?;
      Err(TradingVenueError::NoAccountFound(key.into()))
    }
  }
}

/// Splits a fetch into the SDK protocol accounts, including the exo
/// accounts of `entry_count` registry entries, and the trailing external
/// mints.
fn split_fetched(
  accounts: &[Option<Account>],
  entry_count: usize,
) -> Result<(&[Option<Account>], &[Option<Account>]), TradingVenueError> {
  let protocol_len =
    ProtocolAccounts::PUBKEYS.len() + entry_count * EXO_PAIR_ACCOUNTS;
  accounts
    .split_at_checked(protocol_len)
    .ok_or(TradingVenueError::FailedToFetchMultipleAccountData)
}

/// Registry entries from the exo registry account, the last of
/// [`ProtocolAccounts::PUBKEYS`].
fn registry_entries(
  protocol: &[Option<Account>],
) -> Result<Vec<ExoEntry>, TradingVenueError> {
  let registry = protocol
    .get(ProtocolAccounts::PUBKEYS.len() - 1)
    .and_then(Option::as_ref)
    .ok_or(TradingVenueError::NoAccountFound(pda::EXO_REGISTRY.into()))?;
  read_exo_entries(&registry.data)
    .map_err(|e| TradingVenueError::DeserializationFailed(error_chain(e)))
}

/// Token info for the enshrined mints and every registered exo mint, sorted
/// by mint.
fn token_info(
  protocol_accounts: &ProtocolAccounts,
  ExternalMints {
    jitosol,
    hylosol,
    usdc,
  }: ExternalMints<'_>,
  entries: &[ExoEntry],
  epoch: u64,
) -> Result<Vec<TokenInfo>, TradingVenueError> {
  let exo_mints = entries
    .iter()
    .zip(&protocol_accounts.exo_accounts)
    .flat_map(|(entry, accounts)| {
      [
        (entry.collateral_mint, &accounts.collateral_mint),
        (entry.levercoin_mint, &accounts.levercoin_mint),
      ]
    });
  let mut token_info = [
    (JITOSOL::MINT, jitosol),
    (HYLOSOL::MINT, hylosol),
    (HYUSD::MINT, &protocol_accounts.hyusd_mint),
    (XSOL::MINT, &protocol_accounts.xsol_mint),
    (SHYUSD::MINT, &protocol_accounts.shyusd_mint),
    (USDC::MINT, usdc),
  ]
  .into_iter()
  .chain(exo_mints)
  .map(|(mint, account)| TokenInfo::new(&mint, account, epoch))
  .collect::<Result<Vec<_>, _>>()?;
  token_info.sort_unstable_by_key(|info| info.pubkey);
  token_info.dedup_by_key(|info| info.pubkey);
  Ok(token_info)
}

/// Protocol state and token info from one complete fetch.
fn build_state(
  protocol: &[Option<Account>],
  external: &[Option<Account>],
  entries: &[ExoEntry],
) -> Result<(ProtocolState<Clock>, Vec<TokenInfo>), TradingVenueError> {
  let external = ExternalMints::from_fetched(external)?;
  let protocol_accounts = ProtocolAccounts::from_fetched(protocol)
    .map_err(|e| TradingVenueError::NoAccountFound(error_chain(e)))?;
  let clock: Clock = bincode::deserialize(&protocol_accounts.clock.data)
    .map_err(|e| TradingVenueError::DeserializationFailed(error_chain(e)))?;
  let protocol_state = ProtocolState::try_from(&protocol_accounts)
    .map_err(|e| TradingVenueError::MissingState(error_chain(e)))?;
  let token_info =
    token_info(&protocol_accounts, external, entries, clock.epoch)?;
  Ok((protocol_state, token_info))
}

/// Same registry entries in the same order.
fn entries_match(a: &[ExoEntry], b: &[ExoEntry]) -> bool {
  a.len() == b.len()
    && a.iter().zip(b).all(|(x, y)| {
      x.collateral_mint == y.collateral_mint
        && x.levercoin_mint == y.levercoin_mint
    })
}

/// Hylo V2 exchange venue state.
pub struct HyloRouter {
  pub pool_id: Pubkey,
  pub protocol_state: Option<ProtocolState<Clock>>,
  pub token_info: Vec<TokenInfo>,
  /// Registry entries from the preceding `update_state`.
  pub exo_entries: Vec<ExoEntry>,
  pub initialized: bool,
}

impl HyloRouter {
  /// Quoting state; `NotInitialized` before the first `update_state`.
  fn protocol_state(&self) -> Result<&ProtocolState<Clock>, TradingVenueError> {
    self
      .protocol_state
      .as_ref()
      .ok_or(TradingVenueError::NotInitialized(self.pool_id.into()))
  }
}

impl FromAccount for HyloRouter {
  fn from_account(
    pubkey: &Pubkey,
    account: &Account,
  ) -> Result<Self, TradingVenueError> {
    let exo_entries = match *pubkey {
      pda::HYLO => Hylo::try_deserialize(&mut account.data.as_slice())
        .map(|_| Vec::new())
        .ok(),
      pda::EXO_REGISTRY => read_exo_entries(&account.data).ok(),
      _ => None,
    }
    .ok_or(TradingVenueError::FromAccountError((*pubkey).into()))?;
    Ok(HyloRouter {
      pool_id: *pubkey,
      protocol_state: None,
      token_info: Vec::new(),
      exo_entries,
      initialized: false,
    })
  }
}

#[async_trait]
impl TradingVenue for HyloRouter {
  fn initialized(&self) -> bool {
    self.initialized
  }

  fn program_id(&self) -> Pubkey {
    router::ID_CONST
  }

  fn program_dependencies(&self) -> Vec<Pubkey> {
    vec![
      self.program_id(),
      exchange::ID_CONST,
      earn_pool::ID_CONST,
      TOKEN_PROGRAM_ID,
    ]
  }

  fn directions_num(&self) -> Vec<(u8, u8)> {
    let index = |mint: &Pubkey| {
      self
        .token_info
        .iter()
        .position(|info| info.pubkey == *mint)
        .and_then(|i| u8::try_from(i).ok())
    };
    ENSHRINED_PAIRS
      .iter()
      .copied()
      .chain(self.exo_entries.iter().flat_map(|entry| {
        [
          [entry.collateral_mint, HYUSD::MINT],
          [entry.collateral_mint, entry.levercoin_mint],
          [entry.collateral_mint, USDC::MINT],
          [entry.levercoin_mint, HYUSD::MINT],
        ]
      }))
      .filter_map(|[a, b]| {
        let (a, b) = (index(&a)?, index(&b)?);
        Some([(a, b), (b, a)])
      })
      .flatten()
      .collect()
  }

  /// Protocol-true bounds: the SDK computes the executable input range
  /// from state; no boundary search.
  fn bounds(
    &self,
    tkn_in_ind: u8,
    tkn_out_ind: u8,
  ) -> Result<(u64, u64), TradingVenueError> {
    let input_mint = self.get_token(tkn_in_ind as usize)?.pubkey;
    let output_mint = self.get_token(tkn_out_ind as usize)?.pubkey;
    let state = self.protocol_state()?;
    let lower = state
      .runtime_min_input(input_mint, output_mint)
      .map_err(|e| TradingVenueError::NoQuotableValue(error_chain(e)))?;
    let upper = state
      .runtime_max_input(input_mint, output_mint)
      .map_err(|e| TradingVenueError::NoQuotableValue(error_chain(e)))?;
    Ok((lower, upper))
  }

  fn market_id(&self) -> Pubkey {
    self.pool_id
  }

  fn get_token_info(&self) -> &[TokenInfo] {
    &self.token_info
  }

  fn protocol(&self) -> PoolProtocol {
    PoolProtocol::Hylo
  }

  fn get_required_pubkeys_for_update(
    &self,
  ) -> Result<Vec<Pubkey>, TradingVenueError> {
    let exo_keys = exo_pubkeys_from_entries(&self.exo_entries)?;
    Ok(
      ProtocolAccounts::PUBKEYS
        .into_iter()
        .chain(exo_keys)
        .chain(ExternalMints::PUBKEYS)
        .collect(),
    )
  }

  async fn update_state(
    &mut self,
    cache: &dyn AccountsCache,
  ) -> Result<(), TradingVenueError> {
    // Keys come from the previous registry snapshot; the registry may have
    // changed while the request was in flight.
    let requested_entries = self.exo_entries.clone();
    let keys = self.get_required_pubkeys_for_update()?;
    let accounts = cache.get_accounts(&keys).await?;
    let (protocol, external) =
      split_fetched(&accounts, requested_entries.len())?;
    self.exo_entries = registry_entries(protocol)?;
    if entries_match(&self.exo_entries, &requested_entries) {
      let (protocol_state, token_info) =
        build_state(protocol, external, &self.exo_entries)?;
      self.protocol_state = Some(protocol_state);
      self.token_info = token_info;
      self.initialized = true;
      Ok(())
    } else {
      Box::pin(self.update_state(cache)).await
    }
  }

  fn quote(
    &self,
    QuoteRequest {
      input_mint,
      output_mint,
      amount,
      swap_type,
    }: QuoteRequest,
  ) -> Result<QuoteResult, TradingVenueError> {
    if matches!(swap_type, SwapType::ExactOut) {
      Err(TradingVenueError::ExactOutNotSupported)?;
    }
    let quote = quotes::runtime_quote(
      self.protocol_state()?,
      &self.exo_entries,
      input_mint,
      output_mint,
      amount,
    )?;
    let result = match quote {
      Some(HyloRuntimeQuote {
        expected_output,
        price,
      }) => QuoteResult {
        input_mint,
        output_mint,
        amount,
        expected_output,
        not_enough_liquidity: false,
        price,
      },
      None => QuoteResult {
        input_mint,
        output_mint,
        amount,
        expected_output: 0,
        not_enough_liquidity: true,
        price: 0.0,
      },
    };
    Ok(result)
  }

  fn generate_swap_instruction(
    &self,
    request: QuoteRequest,
    user: Pubkey,
  ) -> Result<Instruction, TradingVenueError> {
    instructions::swap_instruction(&request, user, &self.exo_entries)
  }
}
