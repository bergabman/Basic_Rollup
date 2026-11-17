use serde::Serialize;

/// Local execution is not an L1 settlement proof. No settlement program is configured.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettlementStatus {
    NotConfigured,
}
