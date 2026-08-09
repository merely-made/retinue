//! Version-pinned helper discovery for the public package path.

use crate::executor::{ProcessFailure, ProcessRunner};
use crate::package::{FlashRoute, HelperRequirement};

pub fn version_args(route: &FlashRoute) -> Vec<String> {
    match route {
        FlashRoute::EspRom => vec!["--version".into()],
        FlashRoute::AdafruitDfu => vec!["version".into()],
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
}
