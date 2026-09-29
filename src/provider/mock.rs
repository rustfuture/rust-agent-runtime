//! Deterministic scripted provider. It performs no I/O and spawns no process,
//! so tests can drive the agent loop and worker with an exact action sequence,
//! including provider failures at a chosen step.

use super::{DecisionRequest, ModelAction, ModelDecision, ModelProvider};
use std::{collections::VecDeque, io};

/// Replays a fixed script, one entry per `decide` call, and records every
/// request it receives. An exhausted script is an error rather than a panic.
#[derive(Debug, Default)]
pub struct MockProvider {
    script: VecDeque<io::Result<ModelAction>>,
    requests: Vec<DecisionRequest>,
}

impl MockProvider {
    /// A provider that returns each action in order.
    pub fn from_actions(actions: impl IntoIterator<Item = ModelAction>) -> Self {
        Self {
            script: actions.into_iter().map(Ok).collect(),
            requests: Vec::new(),
        }
    }

    /// A provider whose script may include failures, e.g. a service outage.
    pub fn from_script(script: impl IntoIterator<Item = io::Result<ModelAction>>) -> Self {
        Self {
            script: script.into_iter().collect(),
            requests: Vec::new(),
        }
    }

    /// Requests seen so far, in call order.
    pub fn requests(&self) -> &[DecisionRequest] {
        &self.requests
    }

    /// Script entries not yet consumed.
    pub fn remaining(&self) -> usize {
        self.script.len()
    }
}

impl ModelProvider for MockProvider {
    fn decide(&mut self, request: &DecisionRequest) -> io::Result<ModelDecision> {
        self.requests.push(request.clone());
        let action = self
            .script
            .pop_front()
            .unwrap_or_else(|| Err(io::Error::other("mock provider script exhausted")))?;
        Ok(ModelDecision {
            action,
            model: "mock".to_owned(),
            duration_ms: 0,
            input_tokens: None,
            output_tokens: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> DecisionRequest {
        DecisionRequest {
            task: "t".to_owned(),
            allowed_programs: Vec::new(),
            verification_programs: Vec::new(),
            observations: Vec::new(),
        }
    }

    #[test]
    fn replays_in_order_records_requests_and_reports_exhaustion() {
        let mut provider = MockProvider::from_script([
            Ok(ModelAction::Verify),
            Err(io::Error::other("503 unavailable")),
        ]);
        assert_eq!(
            provider.decide(&request()).unwrap().action,
            ModelAction::Verify
        );
        assert_eq!(
            provider.decide(&request()).unwrap_err().to_string(),
            "503 unavailable"
        );
        assert_eq!(provider.remaining(), 0);
        assert!(provider.decide(&request()).is_err());
        assert_eq!(provider.requests().len(), 3);
    }
}
