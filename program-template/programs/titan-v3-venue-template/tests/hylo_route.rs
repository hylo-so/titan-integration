mod common;

use common::{run_swap_route, RouteConfig};
use hylo_idl::{earn_pool, exchange, pda, router};
use hylo_titan_integration::hylo::HyloRouter;
use litesvm::types::FailedTransactionMetadata;
use solana_pubkey::Pubkey;

/// The router's exo registry (`pda::EXO_REGISTRY`) — the venue's market id.
fn pool() -> Pubkey {
  pda::EXO_REGISTRY
}

fn venue_programs() -> Vec<Pubkey> {
  vec![router::ID_CONST, exchange::ID_CONST, earn_pool::ID_CONST]
}

#[tokio::test]
async fn swap_route_both_directions() -> Result<(), FailedTransactionMetadata> {
  run_swap_route::<HyloRouter>(RouteConfig {
    pool: pool(),
    venue_programs: venue_programs(),
  })
  .await
}
