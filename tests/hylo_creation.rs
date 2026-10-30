use hylo_idl::pda;
use titan_integration_template::hylo::{enshrined_mints, parse_pool_creations};
use titan_integration_template::trading_venue::protocol::PoolProtocol;
use titan_integration_template::trading_venue::venue_creation::PoolCreation;

/// Hylo listings are admin-gated, so the pool list is static: one pool (the
/// global state account) over every routable mint, regardless of input.
#[test]
fn static_pool_list() {
  let expected = vec![PoolCreation {
    protocol: PoolProtocol::Hylo,
    pool: pda::HYLO,
    mints: enshrined_mints(),
  }];
  assert_eq!(parse_pool_creations(&[]), expected);
}
