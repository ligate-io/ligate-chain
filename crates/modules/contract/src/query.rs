//! REST query API for the contract module.
//!
//! Single-key lookups mounted under the module's REST namespace by the
//! runtime via [`HasCustomRestApi`] at `/modules/contract/...`:
//!
//! - `GET /contracts/{contractId}`, fetch one contract by its `lct1…`
//!   Bech32m id.
//! - `GET /contracts/{contractId}/delivery`, fetch the delivery record
//!   (carries `delivered_at_block`, from which the auto-acceptance
//!   deadline `delivered_at_block + dispute_window_blocks` is derived).
//! - `GET /contracts/{contractId}/disputes`, fetch the active dispute
//!   (one per contract at v0).

use std::str::FromStr;

use axum::routing::get;
use serde::{Deserialize, Serialize};
use sov_modules_api::prelude::{axum, UnwrapInfallible};
use sov_modules_api::rest::utils::{errors, ApiResult, Path};
use sov_modules_api::rest::{ApiState, HasCustomRestApi};
use sov_modules_api::{ApiStateAccessor, Spec};

use crate::{ContractId, ContractState, Contracts, DeliveryRecord, DisputeRecord};

/// Body of `GET /contracts/{contractId}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct ContractResponse<S: Spec> {
    /// Contract record as stored on chain.
    pub contract: ContractState<S>,
}

/// Body of `GET /contracts/{contractId}/delivery`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct DeliveryResponse<S: Spec> {
    /// Delivery record as stored on chain.
    pub delivery: DeliveryRecord<S>,
}

/// Body of `GET /contracts/{contractId}/disputes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct ContractDisputeResponse<S: Spec> {
    /// Dispute record as stored on chain.
    pub dispute: DisputeRecord<S>,
}

impl<S: Spec> Contracts<S> {
    async fn route_get_contract(
        state: ApiState<S, Self>,
        mut accessor: ApiStateAccessor<S>,
        Path(contract_id_str): Path<String>,
    ) -> ApiResult<ContractResponse<S>> {
        let contract_id = ContractId::from_str(&contract_id_str).map_err(|e| {
            errors::bad_request_400("Invalid contract id (expected Bech32 `lct1…`)", e.to_string())
        })?;
        let contract = state
            .contracts
            .get(&contract_id, &mut accessor)
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Contract", contract_id))?;
        Ok(ContractResponse { contract }.into())
    }

    /// `GET /contracts/{contractId}/delivery`.
    async fn route_get_delivery(
        state: ApiState<S, Self>,
        mut accessor: ApiStateAccessor<S>,
        Path(contract_id_str): Path<String>,
    ) -> ApiResult<DeliveryResponse<S>> {
        let contract_id = ContractId::from_str(&contract_id_str).map_err(|e| {
            errors::bad_request_400("Invalid contract id (expected Bech32 `lct1…`)", e.to_string())
        })?;
        let delivery = state
            .deliveries
            .get(&contract_id, &mut accessor)
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Delivery", contract_id))?;
        Ok(DeliveryResponse { delivery }.into())
    }

    /// `GET /contracts/{contractId}/disputes`.
    async fn route_get_dispute(
        state: ApiState<S, Self>,
        mut accessor: ApiStateAccessor<S>,
        Path(contract_id_str): Path<String>,
    ) -> ApiResult<ContractDisputeResponse<S>> {
        let contract_id = ContractId::from_str(&contract_id_str).map_err(|e| {
            errors::bad_request_400("Invalid contract id (expected Bech32 `lct1…`)", e.to_string())
        })?;
        let dispute = state
            .disputes
            .get(&contract_id, &mut accessor)
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Dispute", contract_id))?;
        Ok(ContractDisputeResponse { dispute }.into())
    }
}

impl<S: Spec> HasCustomRestApi for Contracts<S> {
    type Spec = S;

    fn custom_rest_api(&self, state: ApiState<S>) -> axum::Router<()> {
        axum::Router::new()
            .route("/contracts/{contractId}", get(Self::route_get_contract))
            .route("/contracts/{contractId}/delivery", get(Self::route_get_delivery))
            .route("/contracts/{contractId}/disputes", get(Self::route_get_dispute))
            .with_state(state.with(self.clone()))
    }
}
