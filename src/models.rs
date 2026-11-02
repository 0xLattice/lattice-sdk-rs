use crate::wire::{Decimal, SafeInteger, present};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    pub phase: String,
    pub chain_path: Vec<String>,
    #[serde(rename = "nexusGenesisCID")]
    pub nexus_genesis_cid: String,
    #[serde(default, deserialize_with = "present", rename = "tipCID")]
    pub tip_cid: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub height: Option<Decimal>,
    #[serde(default, deserialize_with = "present")]
    pub revision: Option<Decimal>,
    pub mempool_count: SafeInteger,
    pub mempool_bytes: SafeInteger,
    #[serde(default, deserialize_with = "present")]
    pub template_digest: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub best_header_height: Option<Decimal>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountState {
    pub owner: String,
    pub balance: Decimal,
    pub nonce: Decimal,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainInfo {
    pub chain: Vec<String>,
    #[serde(default, deserialize_with = "present")]
    pub genesis_hash: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub height: Option<Decimal>,
    #[serde(default, deserialize_with = "present", rename = "tipCID")]
    pub tip_cid: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub min_relay_fee: Option<Decimal>,
    #[serde(default, deserialize_with = "present")]
    pub accepts_submit: Option<bool>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestBlockView {
    pub height: Decimal,
    pub hash: String,
    pub timestamp: Decimal,
    pub transaction_count: SafeInteger,
    #[serde(default, deserialize_with = "present")]
    pub previous_block: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub reward_recipient: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub reward_credited: Option<Decimal>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockView {
    #[serde(flatten)]
    pub summary: LatestBlockView,
    pub child_block_count: SafeInteger,
    pub nonce: Decimal,
    pub version: SafeInteger,
    pub target: String,
    pub next_target: String,
    #[serde(rename = "transactionsCID")]
    pub transactions_cid: String,
    #[serde(rename = "postStateCID")]
    pub post_state_cid: String,
    pub chain: Vec<String>,
}
#[derive(Debug, Deserialize)]
pub struct Mempool {
    pub count: SafeInteger,
    pub transactions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockSummary {
    pub height: Decimal,
    pub hash: String,
    pub timestamp: Decimal,
    pub transaction_count: SafeInteger,
    #[serde(default, deserialize_with = "present")]
    pub previous_block: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub reward_recipient: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlocksPage {
    pub blocks: Vec<BlockSummary>,
    #[serde(default, deserialize_with = "present")]
    pub next_before: Option<Decimal>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockChild {
    pub directory: String,
    pub block_hash: String,
    #[serde(default, deserialize_with = "present")]
    pub height: Option<Decimal>,
    #[serde(default, deserialize_with = "present")]
    pub transaction_count: Option<SafeInteger>,
}
#[derive(Debug)]
pub struct ChildDirectoriesPage {
    pub directories: Vec<String>,
    pub next: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainSpecView {
    pub target_block_time: Decimal,
    pub initial_reward: Decimal,
    pub halving_interval: Decimal,
    pub max_block_size: SafeInteger,
    pub max_number_of_transactions_per_block: Decimal,
    pub premine: Decimal,
    pub half_life: Decimal,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeWeight {
    pub block_room: SafeInteger,
    pub growth_room: SafeInteger,
    pub max_transactions: SafeInteger,
    pub transaction_allowance: SafeInteger,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeEstimates {
    pub min_relay_fee: Decimal,
    pub blocks: Vec<ProjectedBlock>,
    #[serde(default, deserialize_with = "present")]
    pub weight: Option<FeeWeight>,
}
#[derive(Debug, Deserialize)]
#[serde(try_from = "ProjectedBlockWire")]
pub struct ProjectedBlock {
    pub transaction_count: SafeInteger,
    pub bytes: SafeInteger,
    pub lowest_fee: Decimal,
    pub lowest_fee_bytes: SafeInteger,
    pub lowest_fee_weight: SafeInteger,
    pub full: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectedBlockWire {
    transaction_count: SafeInteger,
    bytes: SafeInteger,
    lowest_fee: Decimal,
    lowest_fee_bytes: SafeInteger,
    #[serde(default, deserialize_with = "present")]
    lowest_fee_weight: Option<SafeInteger>,
    full: bool,
}
impl TryFrom<ProjectedBlockWire> for ProjectedBlock {
    type Error = &'static str;
    fn try_from(v: ProjectedBlockWire) -> Result<Self, Self::Error> {
        let weight = v.lowest_fee_weight.unwrap_or(v.lowest_fee_bytes);
        if v.lowest_fee_bytes.0 <= 0 || weight.0 <= 0 {
            return Err("fee rate size and weight must be positive");
        }
        Ok(Self {
            transaction_count: v.transaction_count,
            bytes: v.bytes,
            lowest_fee: v.lowest_fee,
            lowest_fee_bytes: v.lowest_fee_bytes,
            lowest_fee_weight: weight,
            full: v.full,
        })
    }
}
/// Exact integer weight of the most restrictive block resource. Uses i128
/// intermediates, so multiplying safe JSON integers cannot lose precision.
pub fn transaction_weight(
    size: SafeInteger,
    growth: SafeInteger,
    weight: Option<&FeeWeight>,
) -> i128 {
    let Some(w) = weight else {
        return i128::from(size.0).max(1);
    };
    let room = i128::from(w.block_room.0);
    let growth_room = i128::from(w.growth_room.0);
    let mut result = (i128::from(size.0) + i128::from(w.transaction_allowance.0)).max(1);
    if room <= 0 {
        return result;
    }
    if growth.0 > 0 {
        let share = if growth_room <= 0 || i128::from(growth.0) >= growth_room {
            room
        } else {
            (i128::from(growth.0) * room + growth_room - 1) / growth_room
        };
        result = result.max(share);
    }
    if w.max_transactions.0 > 0 {
        result = result.max(room / i128::from(w.max_transactions.0));
    }
    result
}
pub fn fee_to_outbid(block: &ProjectedBlock, weight: SafeInteger) -> Result<Decimal, &'static str> {
    if weight.0 <= 0 || weight.0 > 9_007_199_254_740_991 || block.lowest_fee_weight.0 <= 0 {
        return Err("positive safe weight required");
    }
    Ok(Decimal(
        (&block.lowest_fee.0 * weight.0) / block.lowest_fee_weight.0 + 1,
    ))
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionProjection {
    #[serde(rename = "txCID")]
    pub tx_cid: String,
    #[serde(default, deserialize_with = "present")]
    pub block_height: Option<Decimal>,
    #[serde(default, deserialize_with = "present")]
    pub block_hash: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub timestamp: Option<Decimal>,
    pub nonce: Decimal,
    pub signers: Vec<String>,
    pub chain_path: Vec<String>,
    pub account_actions: Vec<AccountActionView>,
    pub deposit_actions: Vec<DepositActionView>,
    pub receipt_actions: Vec<ReceiptActionView>,
    pub withdrawal_actions: Vec<WithdrawalActionView>,
}
#[derive(Debug, Deserialize)]
pub struct AccountActionView {
    pub owner: String,
    pub delta: Decimal,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositActionView {
    pub nonce: Decimal,
    pub demander: String,
    pub amount_demanded: Decimal,
    pub amount_deposited: Decimal,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptActionView {
    pub withdrawer: String,
    pub nonce: Decimal,
    pub demander: String,
    pub amount_demanded: Decimal,
    pub directory: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawalActionView {
    pub withdrawer: String,
    pub nonce: Decimal,
    pub demander: String,
    pub amount_demanded: Decimal,
    pub amount_withdrawn: Decimal,
}
#[derive(Debug)]
pub struct TransactionContent {
    pub cid: String,
    pub body_cid: String,
}
