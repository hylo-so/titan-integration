mod error;
mod instructions;
mod quotes;

use anchor_lang::{AccountDeserialize, Discriminator};
use async_trait::async_trait;
use hylo_idl::exchange::accounts::Hylo;
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
/// Exo pairs come from the registry at runtime.
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

#[must_use]
pub fn parse_pool_creations(
  instructions: &[ParsedInstruction],
) -> Vec<PoolCreation> {
  let initial = PoolCreation {
    protocol: PoolProtocol::Hylo,
    pool: pda::HYLO,
    mints: enshrined_mints(),
  };
  instructions
    .iter()
    .filter_map(|instruction| {
      let is_registration = instruction.program_id == router::ID
        && instruction
          .data
          .starts_with(router::client::args::RegisterExoEntry::DISCRIMINATOR);
      is_registration
        .then(|| {
          let collateral_mint = *instruction.accounts.get(3)?;
          let levercoin_mint = *instruction.accounts.get(4)?;
          Some(PoolCreation {
            protocol: PoolProtocol::Hylo,
            pool: pda::EXO_REGISTRY,
            mints: vec![
              HYUSD::MINT,
              USDC::MINT,
              collateral_mint,
              levercoin_mint,
            ],
          })
        })
        .flatten()
    })
    .chain(std::iter::once(initial))
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
    let is_hylo_state = *pubkey == pda::HYLO
      && Hylo::try_deserialize(&mut account.data.as_slice()).is_ok();
    let is_exo_registry =
      *pubkey == pda::EXO_REGISTRY && read_exo_entries(&account.data).is_ok();
    if is_hylo_state || is_exo_registry {
      Ok(HyloRouter {
        pool_id: *pubkey,
        protocol_state: None,
        token_info: Vec::new(),
        exo_entries: Vec::new(),
        initialized: false,
      })
    } else {
      Err(TradingVenueError::FromAccountError((*pubkey).into()))
    }
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
    // Account keys are based on the previous registry snapshot. Keep that
    // snapshot for slicing this response; the registry may have changed while
    // the RPC request was in flight.
    let requested_entries = self.exo_entries.clone();
    // Fetch accounts
    let keys = self.get_required_pubkeys_for_update()?;
    let accounts = cache.get_accounts(&keys).await?;

    // Split and validate accounts
    let exo_account_len = requested_entries.len() * EXO_PAIR_ACCOUNTS;
    let (protocol, external) = accounts
      .split_at_checked(ProtocolAccounts::PUBKEYS.len() + exo_account_len)
      .ok_or(TradingVenueError::FailedToFetchMultipleAccountData)?;
    let registry = protocol
      .get(ProtocolAccounts::PUBKEYS.len() - 1)
      .and_then(Option::as_ref)
      .ok_or(TradingVenueError::NoAccountFound(pda::EXO_REGISTRY.into()))?;
    let entries = read_exo_entries(&registry.data)
      .map_err(|e| TradingVenueError::DeserializationFailed(error_chain(e)))?;
    let registry_changed = !entries_match(&entries, &requested_entries);
    self.exo_entries = entries;

    // Update state after every registered pair is available.
    if registry_changed {
      Box::pin(self.update_state(cache)).await
    } else {
      let ExternalMints {
        jitosol,
        hylosol,
        usdc,
      } = ExternalMints::from_fetched(external)?;
      let protocol_accounts = ProtocolAccounts::from_fetched(protocol)
        .map_err(|e| TradingVenueError::NoAccountFound(error_chain(e)))?;
      let clock: Clock = bincode::deserialize(&protocol_accounts.clock.data)
        .map_err(|e| {
          TradingVenueError::DeserializationFailed(error_chain(e))
        })?;
      let epoch = clock.epoch;
      let protocol_state = ProtocolState::try_from(&protocol_accounts)
        .map_err(|e| TradingVenueError::MissingState(error_chain(e)))?;
      let exo_mints = self
        .exo_entries
        .iter()
        .zip(&protocol_accounts.exo_accounts)
        .flat_map(|(entry, accounts)| {
          [
            (entry.collateral_mint, &accounts.collateral_mint),
            (entry.levercoin_mint, &accounts.levercoin_mint),
          ]
        });
      let token_info = [
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

      // Update state
      self.protocol_state = Some(protocol_state);
      self.token_info = token_info;
      self.token_info.sort_unstable_by_key(|info| info.pubkey);
      self.token_info.dedup_by_key(|info| info.pubkey);
      self.initialized = true;
      Ok(())
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
