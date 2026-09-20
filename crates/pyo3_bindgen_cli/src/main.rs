use clap::{error::ErrorKind, Parser};
use pyo3_bindgen_cli::{
    init_observability, run_cli_detailed, run_name_plan, CliArgs, ExitClass, OperationCode,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    init_observability();
    let mut raw_args = std::env::args_os();
    let _program = raw_args.next();
    if raw_args.next().as_deref() == Some(std::ffi::OsStr::new("name-plan")) {
        if raw_args.next().is_some() {
            eprintln!("ERROR[invalid_arguments]: name-plan accepts no command-line arguments");
            return ExitCode::from(ExitClass::Usage.code());
        }
        return match run_name_plan() {
            Ok(()) => ExitCode::from(ExitClass::Success.code()),
            Err(error) => {
                eprintln!("ERROR[invalid_arguments]: {error}");
                ExitCode::from(ExitClass::Usage.code())
            }
        };
    }
    let args = match CliArgs::try_parse() {
        Ok(args) => args,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return ExitCode::from(ExitClass::Success.code());
        }
        Err(error) => {
            eprintln!(
                "ERROR[{}]: command-line arguments were rejected",
                OperationCode::InvalidArguments.as_str()
            );
            let _ = error.print();
            return ExitCode::from(ExitClass::Usage.code());
        }
    };
    let outcome = run_cli_detailed(args);
    if let Some(failure) = outcome.failure() {
        eprintln!(
            "ERROR[{}]: {}",
            failure.primary.code.as_str(),
            failure.primary.message
        );
        for related in &failure.related {
            eprintln!("ERROR[{}]: {}", related.code.as_str(), related.message);
        }
    }
    ExitCode::from(outcome.exit_class().code())
}
