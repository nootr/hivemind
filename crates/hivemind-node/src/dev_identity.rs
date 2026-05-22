use async_trait::async_trait;
use hivemind_app::{AppResult, IdentityPort};
use hivemind_core::{AgentId, AgentKeypair, ObjectBody, ObjectEnvelope};

pub struct DevIdentity {
    keypair: AgentKeypair,
}

impl DevIdentity {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            keypair: AgentKeypair::from_seed(seed),
        }
    }

    pub fn agent_id(&self) -> AgentId {
        self.keypair.agent_id()
    }
}

#[async_trait]
impl IdentityPort for DevIdentity {
    async fn agent_id(&self) -> AppResult<AgentId> {
        Ok(self.keypair.agent_id())
    }

    async fn sign_object(&self, body: ObjectBody) -> AppResult<ObjectEnvelope> {
        Ok(self.keypair.sign_object(body)?)
    }
}
