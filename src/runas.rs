use std::{
    io,
    process::{Command, ExitStatus},
};

pub fn runas(program: &str, command: &str) -> io::Result<ExitStatus> {
    Command::new("powershell")
        .args([
            "-Command",
            &format!("exit (Start-Process -FilePath {program} -Wait -PassThru -WindowStyle Hidden -Verb RunAs -ArgumentList {command}).ExitCode"),
        ])
        .output()
        .map(|e| e.status)
}

pub fn runas_powershell_command(command: &str) -> io::Result<ExitStatus> {
    assert!(!command.contains('"'), "use single quotes instead");
    runas("powershell", &format!(r#""-Command", "{command}""#))
}

#[cfg(test)]
mod test {
    use crate::runas::{runas, runas_powershell_command};

    #[test]
    fn runas_help() {
        let help = runas("powershell", "-Help").unwrap();
        assert!(help.success());
    }

    #[test]
    fn runas_no_help() {
        let help = runas("powershell", "-NoHelp").unwrap();
        assert!(!help.success());
    }

    #[test]
    fn runas_get_help() {
        let help = runas_powershell_command("Get-Help").unwrap();
        assert!(help.success());
    }

    #[test]
    fn runas_get_no_help() {
        let help = runas_powershell_command("Get-No-Help").unwrap();
        assert!(!help.success());
    }
}
