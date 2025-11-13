use crate::artifact::BuildArtifact;
use crate::builder::BuildRequest;
use crate::error::BuildError;

pub mod rust;

pub trait ModuleCompiler {
    fn compile(&self, request: BuildRequest) -> Result<BuildArtifact, BuildError>;
}
