use anchor_lang::AnchorSerialize;
use hylo_titan_integration::swap_route::Venue as RouteBuilderVenue;
use solana_pubkey::Pubkey;
use titan_v3_venue_template::state::Venue as ProgramVenue;

#[test]
fn venue_enum_matches_route_builder() {
  let token_a = Pubkey::new_from_array([3u8; 32]);
  let token_b = Pubkey::new_from_array([4u8; 32]);
  let cases = [
    (
      ProgramVenue::Hylo { token_a, token_b },
      RouteBuilderVenue::Hylo { token_a, token_b },
    ),
    (
      ProgramVenue::Hylo {
        token_a: token_b,
        token_b: token_a,
      },
      RouteBuilderVenue::Hylo {
        token_a: token_b,
        token_b: token_a,
      },
    ),
  ];

  for (program, route_builder) in cases {
    let program_bytes = program.try_to_vec().unwrap();
    let route_builder_bytes = route_builder.to_borsh_bytes();
    assert_eq!(
      program_bytes, route_builder_bytes,
      "Venue {program:?} serializes differently between program and route \
       builder — the two enums have drifted; check that variants match in \
       name and order",
    );
  }
}
