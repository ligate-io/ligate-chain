//! REST query API for the bounty module.
//!
//! v0 single-key lookup mounted under the module's REST namespace by
//! the runtime via [`HasCustomRestApi`] at `/modules/bounty/...`:
//!
//! - `GET /bounties/{bountyId}`, fetch one bounty by its `lbt1…`
//!   Bech32m id.
//!
//! Per-dispute lookup uses the composite (`bounty_id`, `attestation_id`)
//! path; that route lands in the first handler PR alongside the dispute
//! write path. Listing by schema (matching service backend) lives in
//! the indexer per RFC §"Matching", not the chain.

use std::str::FromStr;

use axum::routing::get;
use serde::{Deserialize, Serialize};
use sov_modules_api::prelude::{axum, UnwrapInfallible};
use sov_modules_api::rest::utils::{errors, ApiResult, Path};
use sov_modules_api::rest::{ApiState, HasCustomRestApi};
use sov_modules_api::{ApiStateAccessor, Spec};

use crate::{Bounty, BountyId, BountyState};

// --- response types ----------------------------------------------------------

/// Body of `GET /bounties/{bountyId}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct BountyResponse<S: Spec> {
    /// Bounty record as stored on chain.
    pub bounty: BountyState<S>,
}

// --- routes ------------------------------------------------------------------

impl<S: Spec> Bounty<S> {
    /// `GET /bounties/{bountyId}`, fetch one bounty by Bech32 id.
    async fn route_get_bounty(
        state: ApiState<S, Self>,
        mut accessor: ApiStateAccessor<S>,
        Path(bounty_id_str): Path<String>,
    ) -> ApiResult<BountyResponse<S>> {
        let bounty_id = BountyId::from_str(&bounty_id_str).map_err(|e| {
            errors::bad_request_400("Invalid bounty id (expected Bech32 `lbt1…`)", e.to_string())
        })?;
        let bounty = state
            .bounties
            .get(&bounty_id, &mut accessor)
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Bounty", bounty_id))?;
        Ok(BountyResponse { bounty }.into())
    }
}

// --- router wiring -----------------------------------------------------------

impl<S: Spec> HasCustomRestApi for Bounty<S> {
    type Spec = S;

    fn custom_rest_api(&self, state: ApiState<S>) -> axum::Router<()> {
        axum::Router::new()
            .route("/bounties/{bountyId}", get(Self::route_get_bounty))
            .with_state(state.with(self.clone()))
    }
}
