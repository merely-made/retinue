//! Version-pinned helper discovery for the public package path.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::executor::{ProcessFailure, ProcessRunner};
use crate::package::{FlashRoute, HelperRequirement, sha256_hex};

/// A release assembles helpers beside the executable, under this platform-name
/// directory. It is deliberately a release layout rather than a repository
/// path: the public program must not ask an owner to install Cargo or amend
/// `PATH` before it can flash a verified package.
pub fn bundled_platform_directory() -> String {
    crate::package::helper_platform()
}

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

/// Resolve one package helper before an install begins.
///
/// Installed applications use `<executable-dir>/helpers/<os>-<arch>/`. A
/// staging or CI invocation may supply `LINKBOY_HELPER_DIR`; ambient `PATH` is
/// deliberately unavailable unless a developer explicitly sets
/// `LINKBOY_ALLOW_PATH_HELPERS=1`. The system runner retains the resulting path
/// for every later helper command in the same install.
pub fn resolve_program(program: &str) -> Result<PathBuf, ProcessFailure> {
    let direct = Path::new(program);
    if direct.is_file() {
        return Ok(fs::canonicalize(direct).unwrap_or_else(|_| direct.to_path_buf()));
    }

    let mut directories = Vec::new();
    if let Some(directory) = env::var_os("LINKBOY_HELPER_DIR") {
        directories.push(PathBuf::from(directory));
    }
    if let Some(directory) = installed_helper_directory() {
        directories.push(directory);
    }
    if let Some(executable) = resolve_from_directories(&directories, program) {
        return Ok(executable);
    }

    if env::var_os("LINKBOY_ALLOW_PATH_HELPERS").is_some() {
        let paths = env::var_os("PATH").unwrap_or_default();
        if let Some(executable) =
            resolve_from_directories(&env::split_paths(&paths).collect::<Vec<_>>(), program)
        {
            return Ok(executable);
        }
    }
    Err(ProcessFailure::MissingHelper {
        program: program.into(),
    })
}

/// The helper directory in an installed Linkboy or Signalman application.
pub fn installed_helper_directory() -> Option<PathBuf> {
    env::current_exe()
        .ok()?
        .parent()
        .map(|directory| directory.join("helpers").join(bundled_platform_directory()))
}

fn resolve_from_directories(directories: &[PathBuf], program: &str) -> Option<PathBuf> {
    for directory in directories {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return Some(fs::canonicalize(&candidate).unwrap_or(candidate));
        }
        #[cfg(windows)]
        if Path::new(program).extension().is_none() {
            let executable = candidate.with_extension("exe");
            if executable.is_file() {
                return Some(fs::canonicalize(&executable).unwrap_or(executable));
            }
        }
    }
    None
}

pub fn verify_file_digest(
    path: &Path,
    requirement: &HelperRequirement,
) -> Result<(), ProcessFailure> {
    let Some(expected) = requirement.expected_binary_sha256() else {
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
            expected: expected.to_owned(),
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
        components.clone().next()?;
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
            artifacts: Vec::new(),
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

    #[test]
    fn packaged_helper_directory_uses_the_platform_executable_name() {
        let directory =
            std::env::temp_dir().join(format!("linkboy-packaged-helper-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let helper = if cfg!(windows) {
            directory.join("espflash.exe")
        } else {
            directory.join("espflash")
        };
        std::fs::write(&helper, b"packaged helper").unwrap();

        let found = resolve_from_directories(std::slice::from_ref(&directory), "espflash")
            .expect("packaged helper is resolved without PATH");
        assert_eq!(found, std::fs::canonicalize(&helper).unwrap());
        assert!(bundled_platform_directory().contains(env::consts::OS));

        std::fs::remove_file(helper).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
