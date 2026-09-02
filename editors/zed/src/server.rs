//! Pure logic for locating and launching the Daml language server.
//!
//! This module deliberately contains no `zed_extension_api` calls, so it can be
//! unit tested on a native target. `lib.rs` is the only place that talks to Zed.

use serde::Deserialize;

/// The log levels `damlc multi-ide` accepts. `Telemetry` is deliberately not
/// offered; see `never_enables_telemetry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum LogLevel {
    Debug,
    Info,
    /// The same default as the official VS Code extension.
    #[default]
    Warning,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "Debug",
            LogLevel::Info => "Info",
            LogLevel::Warning => "Warning",
            LogLevel::Error => "Error",
        }
    }
}

/// User settings read from `lsp."daml-language-server".settings`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSettings {
    pub log_level: LogLevel,
    pub extra_arguments: Vec<String>,
}

impl ServerSettings {
    /// Unusable settings fall back to the defaults rather than failing: a typo
    /// in the user's config should not stop the language server from starting.
    pub fn from_json(value: Option<serde_json::Value>) -> Self {
        value
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }
}

/// A resolved command line, independent of Zed's own `Command` type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCommand {
    pub program: String,
    pub args: Vec<String>,
}

pub const INSTALL_HINT: &str = "Daml SDK not found: no `dpm` on PATH. \
Install it, then reopen the project - see https://docs.digitalasset.com/build/3.4/dpm/ - \
or point lsp.\"daml-language-server\".binary.path at it in your Zed settings.";

/// `multi-ide` is always used. It can only be turned off with the legacy Daml
/// Assistant, and this extension targets Daml 3.4+ and dpm.
pub fn build_args(settings: &ServerSettings) -> Vec<String> {
    let mut args = vec![
        "damlc".to_string(),
        "multi-ide".to_string(),
        "--telemetry-ignored".to_string(),
        format!("--log-level={}", settings.log_level.as_str()),
    ];
    args.extend(settings.extra_arguments.iter().cloned());
    args
}

/// `dpm_path` is whatever `worktree.which("dpm")` returned.
pub fn resolve_command(
    dpm_path: Option<String>,
    settings: &ServerSettings,
) -> Result<ResolvedCommand, String> {
    let program = dpm_path.ok_or_else(|| INSTALL_HINT.to_string())?;
    Ok(ResolvedCommand {
        program,
        args: build_args(settings),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_default_arguments() {
        assert_eq!(
            build_args(&ServerSettings::default()),
            vec![
                "damlc".to_string(),
                "multi-ide".to_string(),
                "--telemetry-ignored".to_string(),
                "--log-level=Warning".to_string(),
            ]
        );
    }

    #[test]
    fn honours_log_level() {
        let settings = ServerSettings {
            log_level: LogLevel::Debug,
            ..Default::default()
        };
        assert!(build_args(&settings).contains(&"--log-level=Debug".to_string()));
    }

    #[test]
    fn appends_extra_arguments_last() {
        let settings = ServerSettings {
            extra_arguments: vec!["--ghc-option".into(), "-W".into()],
            ..Default::default()
        };
        let args = build_args(&settings);
        assert_eq!(&args[args.len() - 2..], &["--ghc-option", "-W"]);
    }

    #[test]
    fn never_enables_telemetry() {
        // A Zed extension cannot show a consent dialog, so telemetry stays off.
        let args = build_args(&ServerSettings::default());
        assert!(args.contains(&"--telemetry-ignored".to_string()));
        assert!(!args.contains(&"--telemetry".to_string()));
        assert!(!args.contains(&"--optOutTelemetry".to_string()));
    }

    #[test]
    fn missing_dpm_produces_an_actionable_error() {
        let err = resolve_command(None, &ServerSettings::default()).unwrap_err();
        assert!(err.contains("dpm"));
        assert!(err.contains("https://docs.digitalasset.com"));
    }

    #[test]
    fn uses_dpm_when_found() {
        let cmd =
            resolve_command(Some("/opt/dpm/bin/dpm".into()), &ServerSettings::default()).unwrap();
        assert_eq!(cmd.program, "/opt/dpm/bin/dpm");
        assert_eq!(cmd.args[0], "damlc");
        assert_eq!(cmd.args[1], "multi-ide");
    }

    #[test]
    fn parses_settings_from_json() {
        let value = serde_json::json!({
            "log_level": "Error",
            "extra_arguments": ["--ghc-option", "-Wall"],
        });
        let settings = ServerSettings::from_json(Some(value));
        assert_eq!(settings.log_level, LogLevel::Error);
        assert_eq!(settings.extra_arguments, vec!["--ghc-option", "-Wall"]);
    }

    #[test]
    fn falls_back_to_defaults_on_unusable_settings() {
        // A typo in the user's settings must not stop the server from starting.
        let settings = ServerSettings::from_json(Some(serde_json::json!({"log_level": "Loud"})));
        assert_eq!(settings.log_level, LogLevel::Warning);
        assert!(ServerSettings::from_json(None).extra_arguments.is_empty());
    }
}
