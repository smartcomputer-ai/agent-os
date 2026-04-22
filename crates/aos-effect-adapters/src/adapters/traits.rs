use aos_effects::{EffectIntent, EffectReceipt, EffectStreamFrame};
use async_trait::async_trait;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum EffectUpdate {
    StreamFrame(EffectStreamFrame),
    Receipt(EffectReceipt),
}

pub type EffectUpdateSender = mpsc::Sender<EffectUpdate>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterStartContext {
    pub origin_module_id: String,
    pub origin_workflow_hash: Option<String>,
    pub origin_instance_key: Option<Vec<u8>>,
    pub effect: String,
    pub effect_hash: Option<String>,
    pub executor_module: Option<String>,
    pub executor_module_hash: Option<String>,
    pub executor_entrypoint: Option<String>,
    pub emitted_at_seq: u64,
}

#[async_trait]
pub trait AsyncEffectAdapter: Send + Sync {
    fn kind(&self) -> &str;

    async fn run_terminal(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt>;

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        self.run_terminal(intent).await
    }

    async fn ensure_started(
        &self,
        intent: EffectIntent,
        updates: EffectUpdateSender,
    ) -> anyhow::Result<()> {
        self.ensure_started_with_context(intent, None, updates)
            .await
    }

    async fn ensure_started_with_context(
        &self,
        intent: EffectIntent,
        _context: Option<AdapterStartContext>,
        updates: EffectUpdateSender,
    ) -> anyhow::Result<()> {
        let receipt = self.run_terminal(&intent).await?;
        updates
            .send(EffectUpdate::Receipt(receipt))
            .await
            .map_err(|_| anyhow::anyhow!("effect update receiver dropped"))?;
        Ok(())
    }
}
