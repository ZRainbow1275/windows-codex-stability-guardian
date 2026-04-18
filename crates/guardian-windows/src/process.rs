use std::{
    ffi::OsString,
    io::{self, ErrorKind},
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }
}

pub fn collector_name() -> &'static str {
    "guardian-windows-process"
}

pub fn run_command(program: &str, args: &[OsString]) -> io::Result<CommandOutput> {
    run_command_internal(program, args, false)
}

pub fn run_command_with_cmd_fallback(
    program: &str,
    args: &[OsString],
) -> io::Result<CommandOutput> {
    run_command_internal(program, args, cfg!(target_os = "windows"))
}

fn run_command_internal(
    program: &str,
    args: &[OsString],
    use_cmd_fallback: bool,
) -> io::Result<CommandOutput> {
    let output = match Command::new(program).args(args).output() {
        Ok(output) => output,
        Err(error) if use_cmd_fallback && error.kind() == ErrorKind::NotFound => {
            Command::new("cmd")
                .arg("/C")
                .arg(program)
                .args(args)
                .output()?
        }
        Err(error) => return Err(error),
    };

    Ok(CommandOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: decode_output(&output.stdout).trim().to_string(),
        stderr: decode_output(&output.stderr).trim().to_string(),
    })
}

pub fn decode_output(bytes: &[u8]) -> String {
    let odd_byte_count = bytes.iter().skip(1).step_by(2).count();
    let odd_zero_count = bytes
        .iter()
        .skip(1)
        .step_by(2)
        .filter(|byte| **byte == 0)
        .count();
    let has_utf16_shape = bytes.len() >= 4
        && bytes.len().is_multiple_of(2)
        && odd_zero_count * 4 >= odd_byte_count * 3;
    let decoded = if has_utf16_shape {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };

    decoded.replace('\0', "")
}

#[cfg(test)]
mod tests {
    use super::decode_output;

    #[test]
    fn decodes_utf16_output() {
        let input = "hello"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(decode_output(&input), "hello");
    }

    #[test]
    fn strips_embedded_nulls() {
        assert_eq!(decode_output(b"abc\0def"), "abcdef");
    }
}
