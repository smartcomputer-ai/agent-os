use aos_effects::EffectIntent;
use async_trait::async_trait;

#[async_trait]
pub trait AsyncEffectAdapter: Send + Sync {
    fn kind(&self) -> &str;
    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<aos_effects::EffectReceipt>;
}
