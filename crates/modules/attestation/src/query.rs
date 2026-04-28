//! REST query API for the attestation module.
//!
//! Three single-key lookups exposed under the module's REST namespace
//! (mounted by the runtime via [`HasCustomRestApi`] under
//! `/modules/attestation/...`):
//!
//! - `GET /schemas/{schemaId}` — fetch one schema
//! - `GET /attestor-sets/{attestorSetId}` — fetch one attestor set
//! - `GET /attestations/{attestationId}` — fetch one attestation by
//!   `<schemaId>:<payloadHash>`
//!
//! `list_by_schema` is intentionally **not** in this module. The
//! attestation `StateMap<AttestationId, Attestation<S>>` is keyed by a
//! 64-byte compound id, which is fine for point lookups but bad for
//! filter-by-schema iteration without a secondary index. Adding the
//! index touches the write path on `SubmitAttestation` plus the
//! storage schema, so it ships separately. See the follow-up issue
//! linked from #21.
//!
//! All three routes return a typed JSON body, 404 on missing key, 400
//! on malformed id. Paths use the canonical Bech32 string forms
//! (`lsc1…`, `las1…`, `lsc1…:lph1…`) since those are the strings the
//! protocol exposes everywhere else (genesis files, attestation
//! receipts, error messages).

use std::str::FromStr;

use axum::routing::get;
use serde::{Deserialize, Serialize};
use sov_modules_api::prelude::{axum, UnwrapInfallible};
use sov_modules_api::rest::utils::{errors, ApiResult, Path};
use sov_modules_api::rest::{ApiState, HasCustomRestApi};
use sov_modules_api::{ApiStateAccessor, Spec};

use crate::{
    Attestation, AttestationId, AttestationModule, AttestorSet, AttestorSetId, Schema, SchemaId,
};

// --- response types ----------------------------------------------------------

/// Body of `GET /schemas/{schemaId}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct SchemaResponse<S: Spec> {
    /// The schema as stored on chain.
    pub schema: Schema<S>,
}

/// Body of `GET /attestor-sets/{attestorSetId}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestorSetResponse {
    /// The attestor set as stored on chain.
    pub attestor_set: AttestorSet,
}

/// Body of `GET /attestations/{attestationId}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct AttestationResponse<S: Spec> {
    /// The attestation as stored on chain.
    pub attestation: Attestation<S>,
}

// --- routes ------------------------------------------------------------------

impl<S: Spec> AttestationModule<S> {
    /// `GET /schemas/{schemaId}` — fetch one schema by Bech32 id.
    async fn route_get_schema(
        state: ApiState<S, Self>,
        mut accessor: ApiStateAccessor<S>,
        Path(schema_id_str): Path<String>,
    ) -> ApiResult<SchemaResponse<S>> {
        let schema_id = SchemaId::from_str(&schema_id_str).map_err(|e| {
            errors::bad_request_400("Invalid schema id (expected Bech32 `lsc1…`)", e.to_string())
        })?;
        let schema = state
            .schemas
            .get(&schema_id, &mut accessor)
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Schema", schema_id))?;
        Ok(SchemaResponse { schema }.into())
    }

    /// `GET /attestor-sets/{attestorSetId}` — fetch one attestor set
    /// (members + threshold) by Bech32 id.
    async fn route_get_attestor_set(
        state: ApiState<S, Self>,
        mut accessor: ApiStateAccessor<S>,
        Path(attestor_set_id_str): Path<String>,
    ) -> ApiResult<AttestorSetResponse> {
        let attestor_set_id = AttestorSetId::from_str(&attestor_set_id_str).map_err(|e| {
            errors::bad_request_400(
                "Invalid attestor set id (expected Bech32 `las1…`)",
                e.to_string(),
            )
        })?;
        let attestor_set = state
            .attestor_sets
            .get(&attestor_set_id, &mut accessor)
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("AttestorSet", attestor_set_id))?;
        Ok(AttestorSetResponse { attestor_set }.into())
    }

    /// `GET /attestations/{attestationId}` — fetch one attestation by
    /// its compound id `<schemaId>:<payloadHash>`.
    async fn route_get_attestation(
        state: ApiState<S, Self>,
        mut accessor: ApiStateAccessor<S>,
        Path(attestation_id_str): Path<String>,
    ) -> ApiResult<AttestationResponse<S>> {
        let attestation_id = AttestationId::from_str(&attestation_id_str).map_err(|e| {
            errors::bad_request_400(
                "Invalid attestation id (expected `<schemaId>:<payloadHash>`)",
                e.to_string(),
            )
        })?;
        let attestation = state
            .attestations
            .get(&attestation_id, &mut accessor)
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Attestation", attestation_id))?;
        Ok(AttestationResponse { attestation }.into())
    }
}

// --- router wiring -----------------------------------------------------------

impl<S: Spec> HasCustomRestApi for AttestationModule<S> {
    type Spec = S;

    fn custom_rest_api(&self, state: ApiState<S>) -> axum::Router<()> {
        axum::Router::new()
            .route("/schemas/{schemaId}", get(Self::route_get_schema))
            .route(
                "/attestor-sets/{attestorSetId}",
                get(Self::route_get_attestor_set),
            )
            .route(
                "/attestations/{attestationId}",
                get(Self::route_get_attestation),
            )
            .with_state(state.with(self.clone()))
    }
}
