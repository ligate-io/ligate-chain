//! REST query API for the bounty module.
//!
//! Single-key lookups mounted under the module's REST namespace by the
//! runtime via [`HasCustomRestApi`] at `/modules/bounty/...`:
//!
//! - `GET /bounties/{bountyId}`, fetch one bounty by its `lbt1…`
//!   Bech32m id.
//! - `GET /bounties/{bountyId}/disputes/{attestationId}`, fetch the
//!   active dispute on a specific claim. The two path segments compose
//!   into the [`DisputeKey`] the dispute map is keyed by.
//!
//! Listing by schema (matching service backend) lives in the indexer
//! per RFC §"Matching", not the chain.

use std::str::FromStr;

use attestation::AttestationId;
use axum::routing::get;
use serde::{Deserialize, Serialize};
use sov_modules_api::prelude::{axum, UnwrapInfallible};
use sov_modules_api::rest::utils::{errors, ApiResult, Path};
use sov_modules_api::rest::{ApiState, HasCustomRestApi};
use sov_modules_api::{ApiStateAccessor, Spec};

use crate::{Bounty, BountyId, BountyState, DisputeKey, DisputeState};

// --- response types ----------------------------------------------------------

/// Body of `GET /bounties/{bountyId}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct BountyResponse<S: Spec> {
    /// Bounty record as stored on chain.
    pub bounty: BountyState<S>,
}

/// Body of `GET /bounties/{bountyId}/disputes/{attestationId}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct DisputeResponse<S: Spec> {
    /// Dispute record as stored on chain.
    pub dispute: DisputeState<S>,
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

    /// `GET /bounties/{bountyId}/disputes/{attestationId}`, fetch the
    /// active dispute on one claim. The two path segments compose into
    /// the [`DisputeKey`] the dispute map is keyed by.
    async fn route_get_dispute(
        state: ApiState<S, Self>,
        mut accessor: ApiStateAccessor<S>,
        Path((bounty_id_str, attestation_id_str)): Path<(String, String)>,
    ) -> ApiResult<DisputeResponse<S>> {
        let bounty_id = BountyId::from_str(&bounty_id_str).map_err(|e| {
            errors::bad_request_400("Invalid bounty id (expected Bech32 `lbt1…`)", e.to_string())
        })?;
        let attestation_id = AttestationId::from_str(&attestation_id_str).map_err(|e| {
            errors::bad_request_400(
                "Invalid attestation id (expected Bech32 `lat1…`)",
                e.to_string(),
            )
        })?;
        let key = DisputeKey { bounty_id, attestation_id };
        let dispute = state
            .disputes
            .get(&key, &mut accessor)
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Dispute", key))?;
        Ok(DisputeResponse { dispute }.into())
    }
}

// --- router wiring -----------------------------------------------------------

impl<S: Spec> HasCustomRestApi for Bounty<S> {
    type Spec = S;

    fn custom_rest_api(&self, state: ApiState<S>) -> axum::Router<()> {
        axum::Router::new()
            .route("/bounties/{bountyId}", get(Self::route_get_bounty))
            .route("/bounties/{bountyId}/disputes/{attestationId}", get(Self::route_get_dispute))
            .with_state(state.with(self.clone()))
    }
}
