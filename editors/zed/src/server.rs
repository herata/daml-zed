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
/// Unknown keys are ignored rather than rejected. Rejecting them fails the
/// whole deserialization, which would quietly reset every other setting too -
/// a typo in one key should not silently turn off the others.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    pub log_level: LogLevel,
    pub extra_arguments: Vec<String>,
    /// Evaluate every Daml Script when a file is opened, so a failing script
    /// shows up as a diagnostic instead of staying silent until `dpm test`.
    /// Off by default, as in the VS Code extension: it costs a full evaluation
    /// of every script in the file on every open.
    pub autorun_scripts: bool,
    /// Proxy the language server through daml-ide-bridge, which serves script
    /// results to a browser. Zed cannot render them itself.
    pub script_results: bool,
    /// Where to find the bridge, when it is not on PATH.
    pub bridge_path: Option<String>,
    pub bridge_args: Vec<String>,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            log_level: LogLevel::default(),
            extra_arguments: Vec::new(),
            autorun_scripts: false,
            // Deriving Default would turn this off, which is not the intent.
            script_results: true,
            bridge_path: None,
            bridge_args: Vec::new(),
        }
    }
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
    if settings.autorun_scripts {
        // The flag damlc still expects; the VS Code extension passes the same
        // one and notes that the newer `-all-scripts` spelling is not yet safe
        // to rely on across supported SDKs.
        args.push("--studio-auto-run-all-scenarios=yes".to_string());
    }
    args.extend(settings.extra_arguments.iter().cloned());
    args
}

/// `dpm_path` and `bridge_path` are whatever the lookups in `lib.rs` found.
///
/// With a bridge available the language server runs behind it, which is what
/// makes script results viewable. Without one everything else still works, so
/// a missing bridge is not an error.
pub fn resolve_command(
    dpm_path: Option<String>,
    bridge_path: Option<String>,
    settings: &ServerSettings,
) -> Result<ResolvedCommand, String> {
    let dpm = dpm_path.ok_or_else(|| INSTALL_HINT.to_string())?;
    let server = build_args(settings);

    match bridge_path.filter(|_| settings.script_results) {
        Some(bridge) => {
            let mut args = settings.bridge_args.clone();
            args.push("--".to_string());
            args.push(dpm);
            args.extend(server);
            Ok(ResolvedCommand {
                program: bridge,
                args,
            })
        }
        None => Ok(ResolvedCommand {
            program: dpm,
            args: server,
        }),
    }
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
    fn autorun_is_off_by_default() {
        assert!(!build_args(&ServerSettings::default())
            .iter()
            .any(|a| a.contains("auto-run")));
    }

    #[test]
    fn autorun_turns_script_failures_into_diagnostics() {
        let settings = ServerSettings {
            autorun_scripts: true,
            ..Default::default()
        };
        assert!(build_args(&settings).contains(&"--studio-auto-run-all-scenarios=yes".to_string()));
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
        let err = resolve_command(None, None, &ServerSettings::default()).unwrap_err();
        assert!(err.contains("dpm"));
        assert!(err.contains("https://docs.digitalasset.com"));
    }

    #[test]
    fn uses_dpm_when_found() {
        let cmd = resolve_command(
            Some("/opt/dpm/bin/dpm".into()),
            None,
            &ServerSettings::default(),
        )
        .unwrap();
        assert_eq!(cmd.program, "/opt/dpm/bin/dpm");
        assert_eq!(cmd.args[0], "damlc");
        assert_eq!(cmd.args[1], "multi-ide");
    }

    #[test]
    fn wraps_the_server_in_the_bridge_when_one_is_available() {
        let cmd = resolve_command(
            Some("/opt/dpm/bin/dpm".into()),
            Some("/opt/bin/daml-ide-bridge".into()),
            &ServerSettings::default(),
        )
        .unwrap();
        assert_eq!(cmd.program, "/opt/bin/daml-ide-bridge");
        assert_eq!(cmd.args[0], "--");
        assert_eq!(cmd.args[1], "/opt/dpm/bin/dpm");
        assert_eq!(cmd.args[2], "damlc");
        assert_eq!(cmd.args[3], "multi-ide");
    }

    #[test]
    fn script_results_can_be_turned_off() {
        let settings = ServerSettings {
            script_results: false,
            ..Default::default()
        };
        let cmd = resolve_command(
            Some("/opt/dpm/bin/dpm".into()),
            Some("/opt/bin/daml-ide-bridge".into()),
            &settings,
        )
        .unwrap();
        assert_eq!(cmd.program, "/opt/dpm/bin/dpm");
    }

    #[test]
    fn bridge_arguments_come_before_the_separator() {
        let settings = ServerSettings {
            bridge_args: vec!["--no-open".into()],
            ..Default::default()
        };
        let cmd = resolve_command(
            Some("/opt/dpm/bin/dpm".into()),
            Some("/opt/bin/daml-ide-bridge".into()),
            &settings,
        )
        .unwrap();
        assert_eq!(cmd.args[0], "--no-open");
        assert_eq!(cmd.args[1], "--");
    }

    #[test]
    fn script_results_default_to_on() {
        assert!(ServerSettings::default().script_results);
        assert!(ServerSettings::from_json(Some(serde_json::json!({}))).script_results);
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
    fn a_typo_in_one_key_leaves_the_others_alone() {
        let settings = ServerSettings::from_json(Some(serde_json::json!({
            "autorun_scripts": true,
            "autorun_scripts_typo": true,
        })));
        assert!(settings.autorun_scripts);
    }

    #[test]
    fn falls_back_to_defaults_on_unusable_settings() {
        // A typo in the user's settings must not stop the server from starting.
        let settings = ServerSettings::from_json(Some(serde_json::json!({"log_level": "Loud"})));
        assert_eq!(settings.log_level, LogLevel::Warning);
        assert!(ServerSettings::from_json(None).extra_arguments.is_empty());
    }
}
