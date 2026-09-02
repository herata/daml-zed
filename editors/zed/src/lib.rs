mod server;

use zed_extension_api::{
    self as zed,
    lsp::{Completion, Symbol, SymbolKind},
    settings::LspSettings,
    CodeLabel, CodeLabelSpan, LanguageServerId, Result, Worktree,
};

use crate::server::{resolve_command, ServerSettings};

struct DamlExtension;

impl zed::Extension for DamlExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<zed::Command> {
        let lsp_settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;

        // An explicit binary in the user's settings always wins. This is the
        // escape hatch for the legacy `daml` assistant and for SDKs older than
        // 3.4, which this extension does not detect on its own.
        if let Some(binary) = lsp_settings.binary {
            if let Some(path) = binary.path {
                return Ok(zed::Command {
                    command: path,
                    args: binary.arguments.unwrap_or_default(),
                    env: worktree.shell_env(),
                });
            }
        }

        let settings = ServerSettings::from_json(lsp_settings.settings);
        let bridge = find_bridge(&settings, worktree);
        let resolved = resolve_command(worktree.which("dpm"), bridge, &settings)?;

        Ok(zed::Command {
            command: resolved.program,
            args: resolved.args,
            env: worktree.shell_env(),
        })
    }

    /// Render completions as `name : Type`, using Daml's single-colon
    /// annotation so the grammar highlights the label.
    fn label_for_completion(
        &self,
        _language_server_id: &LanguageServerId,
        completion: Completion,
    ) -> Option<CodeLabel> {
        let detail = completion.detail.as_deref()?.trim();
        if detail.is_empty() {
            return None;
        }
        // damlc sometimes includes the separator itself; normalise it away so
        // exactly one is rendered.
        let detail = detail
            .strip_prefix("::")
            .or_else(|| detail.strip_prefix(':'))
            .unwrap_or(detail)
            .trim();

        let separator = " : ";
        let label = &completion.label;
        let code = format!("{label}{separator}{detail}");
        let detail_start = label.len() + separator.len();

        Some(CodeLabel {
            spans: vec![
                CodeLabelSpan::code_range(0..label.len()),
                CodeLabelSpan::literal(separator, Some("operator".to_string())),
                CodeLabelSpan::code_range(detail_start..code.len()),
            ],
            filter_range: (0..label.len()).into(),
            code,
        })
    }

    fn label_for_symbol(
        &self,
        _language_server_id: &LanguageServerId,
        symbol: Symbol,
    ) -> Option<CodeLabel> {
        let name = &symbol.name;
        let (code, display_range, filter_range) = match symbol.kind {
            SymbolKind::Struct => {
                let prefix = "data ";
                let code = format!("{prefix}{name} = A");
                (
                    code,
                    0..prefix.len() + name.len(),
                    prefix.len()..prefix.len() + name.len(),
                )
            }
            SymbolKind::Constructor => {
                let prefix = "data A = ";
                let code = format!("{prefix}{name}");
                (code, prefix.len()..prefix.len() + name.len(), 0..name.len())
            }
            SymbolKind::Variable | SymbolKind::Function => {
                let code = format!("{name} : T");
                (code, 0..name.len(), 0..name.len())
            }
            _ => return None,
        };

        Some(CodeLabel {
            spans: vec![CodeLabelSpan::code_range(display_range)],
            filter_range: filter_range.into(),
            code,
        })
    }
}

/// Zed cannot show script results itself, so they go through a sidecar. Its
/// absence is not an error: everything else works without it, and the user is
/// simply back to the phase 1 behaviour.
fn find_bridge(settings: &ServerSettings, worktree: &Worktree) -> Option<String> {
    if !settings.script_results {
        return None;
    }
    settings
        .bridge_path
        .clone()
        .or_else(|| worktree.which("daml-ide-bridge"))
}

zed::register_extension!(DamlExtension);
