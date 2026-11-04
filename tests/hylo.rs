mod common;

use common::SuiteConfig;
use hylo_idl::{earn_pool, exchange, pda, router};
use hylo_titan_integration::hylo::HyloRouter;
use litesvm::types::FailedTransactionMetadata;
use solana_pubkey::Pubkey;

// Installs the allocation guard that powers the construction test's
// `assert_no_alloc` checks. The Makefile runs that test under `release-debug`
// so the guard is active; speed tests run under true `--release`.
#[cfg(debug_assertions)]
#[global_allocator]
static A: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// The router's exo registry (`pda::EXO_REGISTRY`) — the venue's market id.
fn pool() -> Pubkey {
  pda::EXO_REGISTRY
}

fn programs() -> Vec<Pubkey> {
  vec![router::ID_CONST, exchange::ID_CONST, earn_pool::ID_CONST]
}

fn config() -> SuiteConfig {
  SuiteConfig {
    pool: pool(),
    programs: programs(),
  }
}

#[tokio::test]
async fn construction() {
  common::construction::<HyloRouter>(&config()).await;
}

#[tokio::test]
async fn zero_input_spot_price() {
  common::zero_input_spot_price::<HyloRouter>(&config()).await;
}

#[tokio::test]
async fn bound_simulation() -> Result<(), FailedTransactionMetadata> {
  common::bound_simulation::<HyloRouter>(&config()).await
}

#[tokio::test]
async fn random_samples() -> Result<(), FailedTransactionMetadata> {
  common::random_samples::<HyloRouter>(&config()).await
}

#[tokio::test]
async fn monotone() {
  common::monotone::<HyloRouter>(&config()).await;
}

#[tokio::test]
async fn quoting_speed() {
  common::quoting_speed::<HyloRouter>(&config()).await;
}

#[tokio::test]
async fn price_monotone() {
  common::price_monotone::<HyloRouter>(&config()).await;
}

#[tokio::test]
async fn mean_value_theorem() {
  common::mean_value_theorem::<HyloRouter>(&config()).await;
}
