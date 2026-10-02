//! Provider boundary.
//!
//! [`ModelProvider`] is the only thing the agent loop knows about a model:
//! given a [`DecisionRequest`] it returns one structured [`ModelDecision`].
//! Everything a provider proposes is still authorized by the runtime executor
//! and workspace editor. [`AgyProvider`] is the included adapter and
//! [`mock::MockProvider`] is a deterministic scripted provider for tests.

mod agy;
mod decision;
pub mod mock;

use crate::executor::CancellationToken;
use std::io;

pub use agy::{AgyProvider, StreamResult};
pub use decision::{DecisionRequest, ModelAction, ModelDecision};

pub trait ModelProvider {
    fn decide(&mut self, request: &DecisionRequest) -> io::Result<ModelDecision>;

    /// Like [`ModelProvider::decide`], but cancellation from the runtime is
    /// propagated to the active provider process. The default implementation
    /// ignores the token for providers that do not spawn a child process.
    fn decide_cancellable(
        &mut self,
        request: &DecisionRequest,
        cancellation: &CancellationToken,
    ) -> io::Result<ModelDecision> {
        let _ = cancellation;
        self.decide(request)
    }
}
