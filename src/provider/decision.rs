use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelAction {
    RunTool {
        program: String,
        #[serde(default)]
        args: Vec<String>,
    },
    ReadFile {
        path: String,
    },
    ReplaceText {
        path: String,
        expected: String,
        replacement: String,
    },
    /// Ask the runtime to run the configured verification command exactly as
    /// configured. The model cannot supply or alter the command.
    Verify,
    Finish {
        summary: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRequest {
    pub task: String,
    pub allowed_programs: Vec<String>,
    pub verification_programs: Vec<String>,
    pub observations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDecision {
    pub action: ModelAction,
    pub model: String,
    pub duration_ms: u128,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

impl Serialize for DecisionRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct View<'a> {
            task: &'a str,
            allowed_programs: &'a [String],
            verification_programs: &'a [String],
            observations: &'a [String],
        }
        View {
            task: &self.task,
            allowed_programs: &self.allowed_programs,
            verification_programs: &self.verification_programs,
            observations: &self.observations,
        }
        .serialize(serializer)
    }
}
