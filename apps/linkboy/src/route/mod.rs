//! Concrete helper command adapters. They build commands and parse helper output; they do not
//! own package compatibility or device identity.

pub mod adafruit_dfu;
pub mod esp_rom;

use crate::executor::ProcessProgress;

pub(crate) fn parse_progress_line(line: &str) -> Option<ProcessProgress> {
    let trimmed = line.trim();
    let before_percent = trimmed.strip_suffix('%')?;
    let digits = before_percent
        .rsplit(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())?;
    let percent = digits.parse::<u64>().ok()?;
    (percent <= 100).then(|| ProcessProgress {
        written: percent,
        total: 100,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_parser_reads_the_last_number_before_percent() {
        assert_eq!(
            parse_progress_line("Writing 50%"),
            Some(ProcessProgress {
                written: 50,
                total: 100,
            })
        );
        assert_eq!(
            parse_progress_line("dfu: 100%"),
            Some(ProcessProgress {
                written: 100,
                total: 100,
            })
        );
        assert_eq!(parse_progress_line("write 101%"), None);
        assert_eq!(parse_progress_line("write complete"), None);
    }
}
