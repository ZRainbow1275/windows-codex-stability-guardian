use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "guardian")]
#[command(version, about = "Windows Codex Stability Guardian", long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Default, Args)]
pub struct GlobalArgs {
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true)]
    pub dry_run: bool,
    #[arg(long, global = true)]
    pub confirm: bool,
    #[arg(long, global = true)]
    pub quiet: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Check(CheckArgs),
    Repair(RepairArgs),
    Diagnose(DiagnoseArgs),
    Export(ExportArgs),
    Gui(GuiArgs),
    Tray(TrayArgs),
}

#[derive(Debug, Clone, Default, Args)]
pub struct CheckArgs {}

#[derive(Debug, Clone, Args)]
pub struct RepairArgs {
    #[command(subcommand)]
    pub target: RepairTarget,
}

#[derive(Debug, Clone, Subcommand)]
pub enum RepairTarget {
    Codex(CodexRepairArgs),
    Docker,
}

#[derive(Debug, Clone, Default, Args)]
pub struct CodexRepairArgs {
    #[arg(long, value_name = "PATH")]
    pub project_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct DiagnoseArgs {
    #[command(subcommand)]
    pub target: DiagnoseTarget,
}

#[derive(Debug, Clone, Subcommand)]
pub enum DiagnoseTarget {
    Profile(DiagnoseProfileArgs),
}

#[derive(Debug, Clone, Default, Args)]
pub struct DiagnoseProfileArgs {
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct ExportArgs {
    #[command(subcommand)]
    pub target: ExportTarget,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ExportTarget {
    Bundle(ExportBundleArgs),
}

#[derive(Debug, Clone, Default, Args)]
pub struct ExportBundleArgs {
    #[arg(long)]
    pub output: Option<PathBuf>,
    #[arg(long)]
    pub zip: bool,
    #[arg(long, value_name = "COUNT", value_parser = clap::value_parser!(usize))]
    pub retain: Option<usize>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct TrayArgs {}

#[derive(Debug, Clone, Default, Args)]
pub struct GuiArgs {}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, CodexRepairArgs, Command, DiagnoseTarget, ExportTarget, RepairTarget};

    #[test]
    fn parses_check_with_global_json_flag() {
        let cli = Cli::parse_from(["guardian", "check", "--json"]);
        assert!(cli.global.json);
        assert!(matches!(cli.command, Command::Check(_)));
    }

    #[test]
    fn parses_repair_codex_dry_run() {
        let cli = Cli::parse_from(["guardian", "repair", "codex", "--dry-run"]);
        assert!(cli.global.dry_run);
        assert!(matches!(
            cli.command,
            Command::Repair(super::RepairArgs {
                target: RepairTarget::Codex(CodexRepairArgs { project_path: None })
            })
        ));
    }

    #[test]
    fn parses_repair_codex_with_project_path_override() {
        let cli = Cli::parse_from([
            "guardian",
            "repair",
            "codex",
            "--project-path",
            "D:\\Desktop\\Inkforge",
            "--confirm",
        ]);
        assert!(cli.global.confirm);
        match cli.command {
            Command::Repair(super::RepairArgs {
                target: RepairTarget::Codex(args),
            }) => {
                assert_eq!(
                    args.project_path.as_deref(),
                    Some(std::path::Path::new("D:\\Desktop\\Inkforge"))
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_nested_diagnose_and_export_commands() {
        let diagnose = Cli::parse_from(["guardian", "diagnose", "profile"]);
        assert!(matches!(
            diagnose.command,
            Command::Diagnose(super::DiagnoseArgs {
                target: DiagnoseTarget::Profile(_)
            })
        ));

        let export = Cli::parse_from([
            "guardian",
            "export",
            "bundle",
            "--output",
            "C:\\temp\\guardian-bundle",
            "--zip",
            "--retain",
            "5",
        ]);
        match export.command {
            Command::Export(super::ExportArgs {
                target: ExportTarget::Bundle(args),
            }) => {
                assert!(args.zip);
                assert_eq!(args.retain, Some(5));
                assert_eq!(
                    args.output.as_deref(),
                    Some(std::path::Path::new("C:\\temp\\guardian-bundle"))
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_gui_command() {
        let gui = Cli::parse_from(["guardian", "gui"]);
        assert!(matches!(gui.command, Command::Gui(_)));
    }
}
