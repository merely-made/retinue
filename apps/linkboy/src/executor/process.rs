use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::package::HelperRequirement;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessProgress {
    pub written: u64,
    pub total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessOutput {
    pub diagnostics: String,
}

pub trait ProcessRunner {
    fn run(
        &mut self,
        program: &str,
        args: &[String],
        progress: &mut dyn FnMut(ProcessProgress),
    ) -> Result<ProcessOutput, ProcessFailure>;

    /// Validate a package-pinned helper before any destructive route stage. Test runners may
    /// leave the default in place because helper availability is injected separately there.
    fn verify_helper(&mut self, _requirement: &HelperRequirement) -> Result<(), ProcessFailure> {
        Ok(())
    }
}

#[derive(Default)]
pub struct SystemProcessRunner {
    verified_helpers: BTreeMap<String, PathBuf>,
}

impl ProcessRunner for SystemProcessRunner {
    fn run(
        &mut self,
        program: &str,
        args: &[String],
        progress: &mut dyn FnMut(ProcessProgress),
    ) -> Result<ProcessOutput, ProcessFailure> {
        let executable = self
            .verified_helpers
            .get(program)
            .cloned()
            // A non-writing loader probe happens before package planning, so it
            // cannot yet have a manifest requirement to verify. It must still
            // use the same installed helper location as the later write rather
            // than silently falling back to PATH.
            .unwrap_or(crate::helper::resolve_program(program)?);
        let output = Command::new(executable)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    ProcessFailure::MissingHelper {
                        program: program.into(),
                    }
                } else {
                    ProcessFailure::Failed {
                        program: program.into(),
                        diagnostics: error.to_string(),
                    }
                }
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let diagnostics = format!("{stdout}{stderr}");
        for line in diagnostics.lines() {
            if let Some(progress_value) = generic_progress(line) {
                progress(progress_value);
            }
        }
        if !output.status.success() {
            return Err(ProcessFailure::Failed {
                program: program.into(),
                diagnostics: diagnostics.trim().to_string(),
            });
        }
        Ok(ProcessOutput { diagnostics })
    }

    fn verify_helper(&mut self, requirement: &HelperRequirement) -> Result<(), ProcessFailure> {
        let executable = crate::helper::resolve_program(&requirement.program)?;
        crate::helper::verify_file_digest(&executable, requirement)?;
        let executable_text = executable.to_string_lossy().into_owned();
        crate::helper::verify_installed_at(self, requirement, &executable_text)?;
        self.verified_helpers
            .insert(requirement.program.clone(), executable);
        Ok(())
    }
}

fn generic_progress(line: &str) -> Option<ProcessProgress> {
    crate::route::parse_progress_line(line)
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ProcessFailure {
    #[error("{program} is not installed")]
    MissingHelper { program: String },
    #[error("{program} failed: {diagnostics}")]
    Failed {
        program: String,
        diagnostics: String,
    },
    #[error("{program} timed out")]
    Timeout { program: String },
    #[error(
        "{program} version mismatch: package requires {expected}, installed helper reports {found}"
    )]
    HelperVersionMismatch {
        program: String,
        expected: String,
        found: String,
    },
    #[error(
        "{program} digest mismatch: package requires {expected}, resolved helper hashes to {found}"
    )]
    HelperDigestMismatch {
        program: String,
        expected: String,
        found: String,
    },
}
