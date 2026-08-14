//! Version-pinned helper discovery for the public package path.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::executor::{ProcessFailure, ProcessRunner};
use crate::package::{FlashRoute, HelperRequirement, sha256_hex};

pub fn version_args(route: &FlashRoute) -> Vec<String> {
    match route {
        FlashRoute::EspRom => vec!["--version".into()],
        FlashRoute::AdafruitDfu => vec!["version".into()],
        FlashRoute::Uf2MassStorage => {
            unreachable!("the built-in UF2 writer has no external helper version command")
        }
    }
}

pub fn verify_installed<P: ProcessRunner>(
    process: &mut P,
    requirement: &HelperRequirement,
) -> Result<(), ProcessFailure> {
    let output = process.run(
        &requirement.program,
        &version_args(&requirement.route),
        &mut |_| {},
    )?;
    let found = parse_version(&output.diagnostics).ok_or_else(|| {
        ProcessFailure::HelperVersionMismatch {
            program: requirement.program.clone(),
            expected: requirement.version.clone(),
            found: output.diagnostics.trim().to_string(),
        }
    })?;
    if found != requirement.version {
        return Err(ProcessFailure::HelperVersionMismatch {
            program: requirement.program.clone(),
            expected: requirement.version.clone(),
            found,
        });
    }
    Ok(())
}

/// Resolve a helper from an explicit path or the current process PATH once, before an install
/// begins. The system runner retains the resulting path for its later write command.
pub fn resolve_program(program: &str) -> Result<PathBuf, ProcessFailure> {
    let direct = Path::new(program);
    if direct.is_file() {
        return Ok(fs::canonicalize(direct).unwrap_or_else(|_| direct.to_path_buf()));
    }

    let Some(paths) = env::var_os("PATH") else {
        return Err(ProcessFailure::MissingHelper {
            program: program.into(),
        });
    };
    for directory in env::split_paths(&paths) {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return Ok(fs::canonicalize(&candidate).unwrap_or(candidate));
        }
        #[cfg(windows)]
        if Path::new(program).extension().is_none() {
            let executable = candidate.with_extension("exe");
            if executable.is_file() {
                return Ok(fs::canonicalize(&executable).unwrap_or(executable));
            }
        }
    }
    Err(ProcessFailure::MissingHelper {
        program: program.into(),
    })
}

pub fn verify_file_digest(
    path: &Path,
    requirement: &HelperRequirement,
) -> Result<(), ProcessFailure> {
    let Some(expected) = &requirement.binary_sha256 else {
        return Ok(());
    };
    let bytes = fs::read(path).map_err(|error| ProcessFailure::Failed {
        program: requirement.program.clone(),
        diagnostics: format!("cannot read resolved helper {}: {error}", path.display()),
    })?;
    let found = sha256_hex(&bytes);
    if !found.eq_ignore_ascii_case(expected) {
        return Err(ProcessFailure::HelperDigestMismatch {
            program: requirement.program.clone(),
            expected: expected.clone(),
            found,
        });
    }
    Ok(())
}

pub fn verify_installed_at<P: ProcessRunner>(
    process: &mut P,
    requirement: &HelperRequirement,
    executable: &str,
) -> Result<(), ProcessFailure> {
    let output = process.run(executable, &version_args(&requirement.route), &mut |_| {})?;
    let found = parse_version(&output.diagnostics).ok_or_else(|| {
        ProcessFailure::HelperVersionMismatch {
            program: requirement.program.clone(),
            expected: requirement.version.clone(),
            found: output.diagnostics.trim().to_string(),
        }
    })?;
    if found != requirement.version {
        return Err(ProcessFailure::HelperVersionMismatch {
            program: requirement.program.clone(),
            expected: requirement.version.clone(),
            found,
        });
    }
    Ok(())
}

fn parse_version(diagnostics: &str) -> Option<String> {
    diagnostics.split_whitespace().find_map(|word| {
        let candidate = word.trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && character != '.' && character != '-'
        });
        let mut components = candidate.split('.');
        let first = components.next()?;
        if first.is_empty() || !first.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
        if components.clone().next().is_none() {
            return None;
        }
        candidate
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
            .then(|| candidate.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{ProcessOutput, ProcessProgress};

    struct MockProcess {
        output: Result<ProcessOutput, ProcessFailure>,
    }

    impl ProcessRunner for MockProcess {
        fn run(
            &mut self,
            _program: &str,
            _args: &[String],
            _progress: &mut dyn FnMut(ProcessProgress),
        ) -> Result<ProcessOutput, ProcessFailure> {
            self.output.clone()
        }
    }

    fn requirement(route: FlashRoute, version: &str) -> HelperRequirement {
        HelperRequirement {
            route,
            program: "helper".into(),
            version: version.into(),
            binary_sha256: None,
            license: "test".into(),
            source_url: "https://example.invalid/helper".into(),
            notice: "test".into(),
        }
    }

    #[test]
    fn helper_commands_use_their_actual_version_interfaces() {
        assert_eq!(version_args(&FlashRoute::EspRom), vec!["--version"]);
        assert_eq!(version_args(&FlashRoute::AdafruitDfu), vec!["version"]);
    }

    #[test]
    fn installed_helper_version_must_match_the_package() {
        let expected = requirement(FlashRoute::EspRom, "4.5.0");
        let mut process = MockProcess {
            output: Ok(ProcessOutput {
                diagnostics: "espflash 4.5.0".into(),
            }),
        };
        assert!(verify_installed(&mut process, &expected).is_ok());

        let expected = requirement(FlashRoute::EspRom, "4.5.0");
        let mut process = MockProcess {
            output: Ok(ProcessOutput {
                diagnostics: "espflash 4.4.0".into(),
            }),
        };
        assert!(matches!(
            verify_installed(&mut process, &expected),
            Err(ProcessFailure::HelperVersionMismatch { .. })
        ));
    }

    #[test]
    fn helper_digest_must_match_the_package() {
        let path =
            std::env::temp_dir().join(format!("linkboy-helper-digest-{}", std::process::id()));
        std::fs::write(&path, b"expected helper").unwrap();
        let mut expected = requirement(FlashRoute::EspRom, "4.5.0");
        expected.binary_sha256 = Some(sha256_hex(b"expected helper"));
        assert!(verify_file_digest(&path, &expected).is_ok());

        expected.binary_sha256 = Some(sha256_hex(b"other helper"));
        assert!(matches!(
            verify_file_digest(&path, &expected),
            Err(ProcessFailure::HelperDigestMismatch { .. })
        ));
        std::fs::remove_file(path).unwrap();
    }
}
