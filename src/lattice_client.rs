//! Compose node claims with CID-verified content. Matching projections and root
//! PoW do not prove canonical membership, child securing work or execution.
use crate::{BlockView, ClientError, NodeClient, TransactionProjection, canonical::*, volumes::*};
use tokio_util::sync::CancellationToken;
#[derive(Debug, thiserror::Error)]
pub enum LatticeClientError {
    #[error(transparent)]
    Node(#[from] ClientError),
    #[error(transparent)]
    Volume(#[from] VolumeError),
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    #[error("node projection differs from verified content: {0}")]
    Mismatch(&'static str),
    #[error("canonical Nexus block fails its own proof-of-work target")]
    ProofOfWork,
    #[error("transaction volume does not contain its body")]
    MissingBody,
}
#[derive(Debug)]
pub struct ContentBlock {
    pub view: BlockView,
    pub volume: VerifiedVolume,
    pub canonical: CanonicalBlock,
}
#[derive(Debug)]
pub struct ContentState {
    pub volume: VerifiedVolume,
    pub canonical: CanonicalLatticeState,
}
#[derive(Debug)]
pub struct ContentTransaction {
    pub projection: TransactionProjection,
    pub transaction: VerifiedVolume,
    pub canonical: CanonicalSignedTransaction,
    pub body: Vec<u8>,
}
pub struct LatticeClient<T> {
    pub node: NodeClient,
    pub volumes: VolumeClient<T>,
}
impl<T: VolumeTransport> LatticeClient<T> {
    pub fn new(node: NodeClient, volumes: VolumeClient<T>) -> Self {
        Self { node, volumes }
    }
    pub async fn latest_block(
        &self,
        cancel: &CancellationToken,
    ) -> Result<ContentBlock, LatticeClientError> {
        let latest = self.node.latest_block(cancel).await?;
        self.block(&latest.hash, cancel).await
    }
    pub async fn block(
        &self,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<ContentBlock, LatticeClientError> {
        let view = self.node.block(id, cancel).await?;
        let volume = self.volumes.get(&view.summary.hash, cancel).await?;
        let canonical = decode_canonical_block(volume.root_bytes())?;
        for (name, matches) in [
            ("height", view.summary.height.0 == canonical.height.into()),
            (
                "timestamp",
                view.summary.timestamp.0 == canonical.timestamp.into(),
            ),
            ("nonce", view.nonce.0 == canonical.nonce.into()),
            ("version", view.version.0 == i64::from(canonical.version)),
            (
                "parent",
                view.summary.previous_block == canonical.parent_cid,
            ),
            ("target", view.target == canonical.target),
            ("next target", view.next_target == canonical.next_target),
            (
                "transactions",
                view.transactions_cid == canonical.transactions_cid,
            ),
            (
                "post-state",
                view.post_state_cid == canonical.post_state_cid,
            ),
            (
                "reward recipient",
                view.summary.reward_recipient == canonical.reward_recipient,
            ),
        ] {
            if !matches {
                return Err(LatticeClientError::Mismatch(name));
            }
        }
        if self.node.chain_path().len() == 1
            && canonical.height > 0
            && !verify_nexus_block_pow(&canonical)?
        {
            return Err(LatticeClientError::ProofOfWork);
        }
        Ok(ContentBlock {
            view,
            volume,
            canonical,
        })
    }
    pub async fn post_state(
        &self,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<ContentState, LatticeClientError> {
        let block = self.block(id, cancel).await?;
        let volume = self
            .volumes
            .get(&block.canonical.post_state_cid, cancel)
            .await?;
        let canonical = decode_canonical_lattice_state(volume.root_bytes())?;
        Ok(ContentState { volume, canonical })
    }
    /// Return a CID-authenticated view rooted at the transaction dictionary.
    /// Its entries belong to the block volume; the dictionary is an ordinary
    /// Header, so it is not independently served by `/volumes/{cid}`.
    pub async fn transactions(
        &self,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<VerifiedVolume, LatticeClientError> {
        let block = self.block(id, cancel).await?;
        Ok(block.volume.with_root(&block.canonical.transactions_cid)?)
    }
    pub async fn transaction(
        &self,
        cid: &str,
        cancel: &CancellationToken,
    ) -> Result<ContentTransaction, LatticeClientError> {
        let (projection, transaction) = tokio::try_join!(
            async {
                self.node
                    .transaction(cid, cancel)
                    .await
                    .map_err(LatticeClientError::from)
            },
            async {
                self.volumes
                    .get(cid, cancel)
                    .await
                    .map_err(LatticeClientError::from)
            },
        )?;
        if projection.tx_cid != cid {
            return Err(LatticeClientError::Mismatch("transaction CID"));
        }
        let canonical = decode_canonical_signed_transaction(transaction.root_bytes())?;
        let body = transaction
            .bytes(&canonical.body_cid)
            .ok_or(LatticeClientError::MissingBody)?
            .to_vec();
        Ok(ContentTransaction {
            projection,
            transaction,
            canonical,
            body,
        })
    }
}
