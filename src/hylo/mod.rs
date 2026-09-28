mod error;
mod instructions;
mod quotes;

use anchor_lang::{AccountDeserialize, Discriminator};
use async_trait::async_trait;
use hylo_idl::exchange::accounts::{ExoPair, Hylo};
use hylo_idl::tokens::{
  HYLOSOL, HYUSD, JITOSOL, SHYUSD, TokenMint, USDC, XSOL,
};
use hylo_idl::{earn_pool, exchange, pda, router};
use hylo_quotes::prelude::ProtocolState;
use hylo_quotes::protocol_state::ProtocolAccounts;
use solana_account::Account;
use solana_instruction::Instruction;
use solana_program::clock::Clock;
use solana_pubkey::Pubkey;

use self::error::error_chain;
use self::quotes::RuntimeQuote;
use crate::account_caching::AccountsCache;
use crate::trading_venue::error::TradingVenueError;
use crate::trading_venue::protocol::PoolProtocol;
use crate::trading_venue::token_info::{TOKEN_PROGRAM_ID, TokenInfo};
use crate::trading_venue::venue_creation::{ParsedInstruction, PoolCreation};
use crate::trading_venue::{
  FromAccount, QuoteRequest, QuoteResult, SwapType, TradingVenue,
};

/// Bidirectional swap pairs supported by Hylo router.
pub const PAIRS: [[Pubkey; 2]; 10] = [
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

/// Unique mints appearing in [`PAIRS`].
#[must_use]
pub fn pair_mints() -> Vec<Pubkey> {
  PAIRS
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
    mints: pair_mints(),
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

fn exo_accounts_in_fetch_order(
  dynamic: &[Option<Account>],
  oracle_accounts: &[Option<Account>],
) -> Vec<Option<Account>> {
  dynamic
    .chunks_exact(4)
    .zip(oracle_accounts.iter())
    .flat_map(|(pair_accounts, oracle)| {
      pair_accounts
        .iter()
        .cloned()
        .chain(std::iter::once(oracle.clone()))
    })
    .collect()
}

/// Hylo V2 exchange venue state.
pub struct HyloRouter {
  pub pool_id: Pubkey,
  pub protocol_state: Option<ProtocolState<Clock>>,
  pub token_info: Vec<TokenInfo>,
  pub exo_pairs: Vec<[Pubkey; 2]>,
  /// Oracle accounts learned from the preceding EXO-pair refresh.
  pub exo_oracles: Vec<[Pubkey; 2]>,
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
    let is_exo_registry = *pubkey == pda::EXO_REGISTRY
      && account
        .data
        .get(router::accounts::ExoRegistry::DISCRIMINATOR.len()..)
        .and_then(|data| {
          bytemuck::try_pod_read_unaligned::<router::accounts::ExoRegistry>(
            data,
          )
          .ok()
        })
        .is_some();
    if is_hylo_state || is_exo_registry {
      Ok(HyloRouter {
        pool_id: *pubkey,
        protocol_state: None,
        token_info: Vec::new(),
        exo_pairs: Vec::new(),
        exo_oracles: Vec::new(),
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
    PAIRS
      .iter()
      .copied()
      .chain(self.exo_pairs.iter().flat_map(|[collateral, levercoin]| {
        [
          [*collateral, HYUSD::MINT],
          [*collateral, *levercoin],
          [*collateral, USDC::MINT],
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
    Ok(
      ProtocolAccounts::PUBKEYS
        .into_iter()
        .chain(self.exo_pairs.iter().flat_map(|[collateral, levercoin]| {
          [
            pda::exo_pair(*collateral),
            pda::exo_vault(*collateral),
            *levercoin,
            *collateral,
          ]
        }))
        .chain(self.exo_oracles.iter().map(|[_, oracle]| *oracle))
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
    let requested_exo_pairs = self.exo_pairs.clone();
    // Fetch accounts
    let keys = self.get_required_pubkeys_for_update()?;
    let accounts = cache.get_accounts(&keys).await?;

    // Split and validate accounts
    let (protocol, external) = accounts
      .split_at_checked(ProtocolAccounts::PUBKEYS.len())
      .ok_or(TradingVenueError::FailedToFetchMultipleAccountData)?;
    let registry = protocol
      .last()
      .ok_or(TradingVenueError::FailedToFetchMultipleAccountData)?
      .as_ref()
      .ok_or(TradingVenueError::NoAccountFound(pda::EXO_REGISTRY.into()))?;
    let registry_data = registry
      .data
      .get(router::accounts::ExoRegistry::DISCRIMINATOR.len()..)
      .ok_or(TradingVenueError::DeserializationFailed(
        "EXO registry discriminator missing".into(),
      ))?;
    let registry: router::accounts::ExoRegistry =
      bytemuck::try_pod_read_unaligned(registry_data).map_err(|error| {
        TradingVenueError::DeserializationFailed(error.to_string().into())
      })?;
    let exo_len = usize::from(registry.current_size);
    let registry_entries = registry.entries.get(..exo_len).ok_or(
      TradingVenueError::DeserializationFailed(
        "EXO registry length exceeds capacity".into(),
      ),
    )?;
    self.exo_pairs = registry_entries
      .iter()
      .map(|entry| [entry.collateral_mint, entry.levercoin_mint])
      .collect();
    let dynamic_account_len = requested_exo_pairs.len() * 4;
    let (dynamic, external) = external
      .split_at_checked(dynamic_account_len)
      .ok_or(TradingVenueError::FailedToFetchMultipleAccountData)?;
    let (oracle_accounts, external) = external
      .split_at_checked(self.exo_oracles.len())
      .ok_or(TradingVenueError::FailedToFetchMultipleAccountData)?;
    let ExternalMints {
      jitosol,
      hylosol,
      usdc,
    } = ExternalMints::from_fetched(external)?;

    // Read each registered pair to discover its Pyth feed.
    let discovered = requested_exo_pairs
      .iter()
      .zip(dynamic.chunks_exact(4))
      .filter_map(|([collateral, levercoin], accounts)| match accounts {
        [
          Some(exo_pair),
          Some(_vault),
          Some(levercoin_mint),
          Some(collateral_mint),
        ] => Some((
          collateral,
          levercoin,
          exo_pair,
          levercoin_mint,
          collateral_mint,
        )),
        _ => None,
      })
      .map(
        |(collateral, levercoin, exo_pair, levercoin_mint, collateral_mint)| {
          let pair = ExoPair::try_deserialize(&mut exo_pair.data.as_slice())
            .map_err(|error| {
              TradingVenueError::DeserializationFailed(error_chain(error))
            })?;
          Ok((
            [*collateral, pair.oracle],
            [(*collateral, collateral_mint), (*levercoin, levercoin_mint)],
          ))
        },
      )
      .collect::<Result<Vec<_>, TradingVenueError>>()?;
    let (discovered_oracles, dynamic_mint_groups): (Vec<_>, Vec<_>) =
      discovered.into_iter().unzip();
    let dynamic_mints: Vec<_> =
      dynamic_mint_groups.into_iter().flatten().collect();
    self.exo_oracles = discovered_oracles;

    // Update state after every registered pair and oracle is available.
    if requested_exo_pairs.len() == registry_entries.len()
      && oracle_accounts.len() == registry_entries.len()
    {
      let protocol_with_exo = protocol
        .iter()
        .cloned()
        .chain(exo_accounts_in_fetch_order(dynamic, oracle_accounts))
        .collect::<Vec<_>>();
      let protocol_accounts =
        ProtocolAccounts::from_fetched(&protocol_with_exo)
          .map_err(|e| TradingVenueError::NoAccountFound(error_chain(e)))?;
      let clock: Clock = bincode::deserialize(&protocol_accounts.clock.data)
        .map_err(|e| {
          TradingVenueError::DeserializationFailed(error_chain(e))
        })?;
      let epoch = clock.epoch;
      let protocol_state = ProtocolState::try_from(&protocol_accounts)
        .map_err(|e| TradingVenueError::MissingState(error_chain(e)))?;

      // Update state
      self.protocol_state = Some(protocol_state);
      self.token_info = vec![
        TokenInfo::new(&JITOSOL::MINT, jitosol, epoch)?,
        TokenInfo::new(&HYLOSOL::MINT, hylosol, epoch)?,
        TokenInfo::new(&HYUSD::MINT, &protocol_accounts.hyusd_mint, epoch)?,
        TokenInfo::new(&XSOL::MINT, &protocol_accounts.xsol_mint, epoch)?,
        TokenInfo::new(&SHYUSD::MINT, &protocol_accounts.shyusd_mint, epoch)?,
        TokenInfo::new(&USDC::MINT, usdc, epoch)?,
      ];
      self.token_info.extend(
        dynamic_mints
          .into_iter()
          .map(|(mint, account)| TokenInfo::new(&mint, account, epoch))
          .collect::<Result<Vec<_>, _>>()?,
      );
      self.token_info.sort_unstable_by_key(|info| info.pubkey);
      self.token_info.dedup_by_key(|info| info.pubkey);
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
      &self.exo_pairs,
      input_mint,
      output_mint,
      amount,
    )?;
    let result = match quote {
      Some(RuntimeQuote {
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
    instructions::swap_instruction(
      &request,
      user,
      &self.exo_pairs,
      &self.exo_oracles,
    )
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn exo_accounts_interleave_oracles_per_registry_entry() {
    let dynamic = (1..=8)
      .map(|lamports| {
        Some(Account {
          lamports,
          ..Account::default()
        })
      })
      .collect::<Vec<_>>();
    let oracles = (9..=10)
      .map(|lamports| {
        Some(Account {
          lamports,
          ..Account::default()
        })
      })
      .collect::<Vec<_>>();

    let lamports = exo_accounts_in_fetch_order(&dynamic, &oracles)
      .iter()
      .map(|account| account.as_ref().map(|account| account.lamports))
      .collect::<Vec<_>>();

    assert_eq!(
      lamports,
      vec![
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(9),
        Some(5),
        Some(6),
        Some(7),
        Some(8),
        Some(10)
      ]
    );
  }
}
