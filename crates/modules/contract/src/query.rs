//! REST query API for the contract module.
//!
//! v0 single-key lookup mounted under the module's REST namespace by
//! the runtime via [`HasCustomRestApi`] at `/modules/contract/...`:
//!
//! - `GET /contracts/{contractId}`, fetch one contract by its
//!   `lct1…` Bech32m id.

use std::str::FromStr;

use axum::routing::get;
use serde::{Deserialize, Serialize};
use sov_modules_api::prelude::{axum, UnwrapInfallible};
use sov_modules_api::rest::utils::{errors, ApiResult, Path};
use sov_modules_api::rest::{ApiState, HasCustomRestApi};
use sov_modules_api::{ApiStateAccessor, Spec};

use crate::{ContractId, ContractState, Contracts};

/// Body of `GET /contracts/{contractId}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct ContractResponse<S: Spec> {
    /// Contract record as stored on chain.
    pub contract: ContractState<S>,
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
}

impl<S: Spec> HasCustomRestApi for Contracts<S> {
    type Spec = S;

    fn custom_rest_api(&self, state: ApiState<S>) -> axum::Router<()> {
        axum::Router::new()
            .route("/contracts/{contractId}", get(Self::route_get_contract))
            .with_state(state.with(self.clone()))
    }
}
