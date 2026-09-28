#![allow(clippy::wildcard_imports)]

use std::fmt::Display;

use hylo_core::error::CoreError;
use hylo_core::error::CoreError::*;
use hylo_idl::pda;

use crate::trading_venue::error::{ErrorInfo, TradingVenueError};
use crate::trading_venue::protocol::PoolProtocol;

/// Formats an error with its cause chain.
pub fn error_chain(error: impl Display) -> ErrorInfo {
  format!("{error:#}").into()
}

/// Swap size exceeds protocol liquidity or limits.
pub fn exceeds_liquidity(error: CoreError) -> bool {
  matches!(
    error,
    InsufficientLiquidity
      | InsufficientEarnPoolLiquidity
      | RebalanceSellSideLiquidity
      | RebalanceBuySideTarget
      | RebalanceBuyTargetExceeded
      | RebalanceAmountExceeded
      | RebalanceOutOfDomain
      | RequestedStablecoinOverMaxMintable
      | VirtualStablecoinBurnLimit
      | BurnUnderflow
      | MintOverflow
      | DestinationCollateral
      | DestinationStablecoin
      | NoValidStablecoinMintFee
      | LevercoinMarketCapLimitReached
      | LevercoinMarketCapExceeded
      | DepositLimitExceeded
      | WithdrawalLimitExceededForEpoch
  )
}

/// Pair or protocol disabled.
fn pool_inactive(error: CoreError) -> bool {
  matches!(
    error,
    ProtocolPaused
      | PairPaused
      | OperationDisabled
      | DrawdownNotRepaid
      | NoValidLevercoinMintFee
      | NoValidLevercoinRedeemFee
      | NoValidSwapFee
  )
}

/// Crank or oracle stale.
fn state_stale(error: CoreError) -> bool {
  matches!(
    error,
    TotalSolCacheOutdated
      | LstSolPriceOutdated
      | LstSolPriceEpochOrder
      | YieldHarvestNotRun
      | BorrowRateHarvestNotRun
      | WithdrawalLimitInvalidEpoch
      | LevercoinSupplyNotSet
      | PythOracleOutdated
      | PythOracleSlotInvalid
      | PythOracleConfidence
      | PythOracleVerificationLevel
  )
}

/// Numeric conversion failed.
fn conversion_failed(error: CoreError) -> bool {
  matches!(
    error,
    TokenAmountPrecision
      | FixValueConversion
      | ExoAmountNormalization
      | PythOracleExponent
      | PythOracleNegativePrice
      | PythOracleNegativeTime
      | PythOraclePriceRange
      | InterpFeeConversion
      | CollateralRatioConversion
      | RebalancePriceConversion
  )
}

/// Invalid protocol config.
fn config_invalid(error: CoreError) -> bool {
  matches!(
    error,
    InvalidFees
      | YieldHarvestConfigValidation
      | OracleIntervalSecsInvalid
      | OracleConfToleranceInvalid
      | StablecoinMintThresholdInvalid
      | LevercoinMarketCapLimitInvalid
      | DepositLimitValidation
      | WithdrawalLimitValidation
      | RebalanceCurveConfigValidation
      | InterpInsufficientPoints
      | InterpPointsNotMonotonic
      | BorrowRateValidation
      | RangeUnexpectedBound
      | TargetCollateralRatioTooLow
  )
}

/// Checked arithmetic failed.
fn math_failed(error: CoreError) -> bool {
  matches!(
    error,
    TotalSolCacheIncrement
      | TotalSolCacheOverflow
      | TotalSolCacheUnderflow
      | LstSolPriceDelta
      | LstSolPriceConversion
      | SolLstPriceConversion
      | LstLstPriceConversion
      | MaxMintable
      | MaxSwappable
      | StablecoinNav
      | TotalValueLocked
      | SlippageArithmetic
      | LeverToStable
      | StableToLever
      | LstToToken
      | TokenToLst
      | FeeExtraction
      | LevercoinNav
      | LpTokenNav
      | LpTokenOut
      | TokenWithdraw
      | VirtualStablecoinOverhang
      | VirtualStablecoinSurplus
      | InterpArithmetic
      | MarginalRateInvalid
      | BorrowRateApply
      | ExoToToken
      | ExoFromToken
      | ExoCollateralToUsdc
      | ExoUsdcToCollateral
      | LstToUsdc
      | UsdcToLst
      | RebalancePriceConstruction
      | RebalancePercentArithmetic
      | RebalanceSwapPnl
      | StakePoolDivByZero
      | LevercoinMarketCapArithmetic
      | DepositLimitArithmetic
      | WithdrawalLimitArithmetic
  )
}

impl From<CoreError> for TradingVenueError {
  fn from(error: CoreError) -> Self {
    let info = ErrorInfo::from(error.to_string());
    if pool_inactive(error) {
      TradingVenueError::InactivePoolError(pda::HYLO, PoolProtocol::Hylo)
    } else if state_stale(error) {
      TradingVenueError::MissingState(info)
    } else if matches!(error, StakePoolAccountData) {
      TradingVenueError::DeserializationFailed(info)
    } else if matches!(error, ProtocolAccountNotFound) {
      TradingVenueError::NoAccountFound(info)
    } else if matches!(error, UnknownLstMint) {
      TradingVenueError::InvalidMint(info)
    } else if conversion_failed(error) {
      TradingVenueError::DataConversionError(info)
    } else if config_invalid(error) {
      TradingVenueError::UnsupportedVenue(info)
    } else if math_failed(error) {
      TradingVenueError::CheckedMathError(info)
    } else {
      TradingVenueError::AmmMethodError(info)
    }
  }
}
