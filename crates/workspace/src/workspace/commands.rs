#[derive(Clone, Copy)]
pub(crate) struct CommandEntry {
    pub(crate) label_key: &'static str,
    pub(crate) action_name: &'static str,
}

/// Command palette の登録境界。action id と表示キーの対応を shell 本体から分離する。
pub(crate) struct CommandRegistry;

impl CommandRegistry {
    pub(crate) fn entries(&self) -> &'static [CommandEntry] {
        &[
            CommandEntry {
                label_key: "cmd.file_finder",
                action_name: "workspace::FileFinder",
            },
            CommandEntry {
                label_key: "cmd.save_active",
                action_name: "workspace::SaveActive",
            },
            CommandEntry {
                label_key: "cmd.close_tab",
                action_name: "workspace::CloseTab",
            },
            CommandEntry {
                label_key: "cmd.restore_closed_tab",
                action_name: "workspace::RestoreClosedTab",
            },
            CommandEntry {
                label_key: "cmd.project_switcher",
                action_name: "workspace::ProjectSwitcher",
            },
            CommandEntry {
                label_key: "cmd.next_project",
                action_name: "workspace::NextProject",
            },
            CommandEntry {
                label_key: "cmd.prev_project",
                action_name: "workspace::PrevProject",
            },
            CommandEntry {
                label_key: "cmd.new_window",
                action_name: "workspace::NewWindow",
            },
            CommandEntry {
                label_key: "cmd.buffer_search",
                action_name: "workspace::BufferSearch",
            },
            CommandEntry {
                label_key: "cmd.buffer_replace",
                action_name: "workspace::BufferReplace",
            },
            CommandEntry {
                label_key: "cmd.project_search",
                action_name: "workspace::ProjectSearch",
            },
            CommandEntry {
                label_key: "cmd.find_references",
                action_name: "workspace::FindReferences",
            },
            CommandEntry {
                label_key: "cmd.go_to_line",
                action_name: "workspace::GoToLine",
            },
            CommandEntry {
                label_key: "cmd.go_to_definition",
                action_name: "workspace::GoToDefinition",
            },
            CommandEntry {
                label_key: "cmd.navigate_back",
                action_name: "workspace::NavigateBack",
            },
            CommandEntry {
                label_key: "cmd.navigate_forward",
                action_name: "workspace::NavigateForward",
            },
            CommandEntry {
                label_key: "cmd.outline_symbols",
                action_name: "workspace::OutlineSymbols",
            },
            CommandEntry {
                label_key: "cmd.workspace_symbols",
                action_name: "workspace::WorkspaceSymbols",
            },
            CommandEntry {
                label_key: "cmd.next_diagnostic",
                action_name: "workspace::NextDiagnostic",
            },
            CommandEntry {
                label_key: "cmd.prev_diagnostic",
                action_name: "workspace::PrevDiagnostic",
            },
            CommandEntry {
                label_key: "cmd.diagnostics_panel",
                action_name: "workspace::DiagnosticsPanel",
            },
            CommandEntry {
                label_key: "cmd.format",
                action_name: "workspace::Format",
            },
            CommandEntry {
                label_key: "cmd.rename",
                action_name: "workspace::Rename",
            },
            CommandEntry {
                label_key: "cmd.code_actions",
                action_name: "workspace::CodeActions",
            },
            CommandEntry {
                label_key: "cmd.inline_edit",
                action_name: "workspace::InlineEdit",
            },
            CommandEntry {
                label_key: "cmd.trigger_completion",
                action_name: "workspace::TriggerCompletion",
            },
            CommandEntry {
                label_key: "cmd.show_hover",
                action_name: "workspace::ShowHover",
            },
            CommandEntry {
                label_key: "cmd.open_diff",
                action_name: "workspace::OpenDiff",
            },
            CommandEntry {
                label_key: "cmd.open_review",
                action_name: "workspace::OpenReview",
            },
            CommandEntry {
                label_key: "cmd.next_hunk",
                action_name: "workspace::NextHunk",
            },
            CommandEntry {
                label_key: "cmd.prev_hunk",
                action_name: "workspace::PrevHunk",
            },
            CommandEntry {
                label_key: "cmd.theme_selector",
                action_name: "workspace::ThemeSelector",
            },
            CommandEntry {
                label_key: "cmd.project_color",
                action_name: "workspace::ProjectColor",
            },
            CommandEntry {
                label_key: "cmd.toggle_terminal",
                action_name: "workspace::ToggleTerminal",
            },
            CommandEntry {
                label_key: "cmd.toggle_git_panel",
                action_name: "workspace::ToggleGitPanel",
            },
            CommandEntry {
                label_key: "cmd.toggle_todo_board",
                action_name: "workspace::ToggleTodoBoard",
            },
            CommandEntry {
                label_key: "cmd.toggle_fleet",
                action_name: "workspace::ToggleFleet",
            },
            CommandEntry {
                label_key: "cmd.toggle_chat",
                action_name: "workspace::ToggleChat",
            },
            CommandEntry {
                label_key: "cmd.new_chat",
                action_name: "workspace::NewChat",
            },
            CommandEntry {
                label_key: "cmd.focus_captain",
                action_name: "workspace::FocusCaptain",
            },
            CommandEntry {
                label_key: "cmd.toggle_agent_full_screen",
                action_name: "workspace::ToggleAgentFullScreen",
            },
            CommandEntry {
                label_key: "cmd.split_right",
                action_name: "workspace::SplitRight",
            },
            CommandEntry {
                label_key: "cmd.open_localhost_preview",
                action_name: "workspace::OpenLocalhostPreview",
            },
            CommandEntry {
                label_key: "cmd.toggle_design_mode",
                action_name: "workspace::ToggleDesignMode",
            },
            CommandEntry {
                label_key: "cmd.new_thread",
                action_name: "workspace::NewThread",
            },
            CommandEntry {
                label_key: "cmd.new_thread_claude",
                action_name: "workspace::NewThreadClaudeCode",
            },
            CommandEntry {
                label_key: "cmd.new_thread_codex",
                action_name: "workspace::NewThreadCodex",
            },
            CommandEntry {
                label_key: "cmd.new_thread_copilot",
                action_name: "workspace::NewThreadCopilot",
            },
            CommandEntry {
                label_key: "cmd.new_thread_qwen",
                action_name: "workspace::NewThreadQwenCode",
            },
            CommandEntry {
                label_key: "cmd.new_thread_opencode",
                action_name: "workspace::NewThreadOpenCode",
            },
            CommandEntry {
                label_key: "cmd.new_thread_kimi",
                action_name: "workspace::NewThreadKimi",
            },
            CommandEntry {
                label_key: "cmd.new_thread_grok",
                action_name: "workspace::NewThreadGrok",
            },
            CommandEntry {
                label_key: "cmd.next_tab",
                action_name: "workspace::SelectNextTab",
            },
            CommandEntry {
                label_key: "cmd.prev_tab",
                action_name: "workspace::SelectPrevTab",
            },
            CommandEntry {
                label_key: "cmd.next_thread",
                action_name: "workspace::SelectNextThread",
            },
            CommandEntry {
                label_key: "cmd.prev_thread",
                action_name: "workspace::SelectPrevThread",
            },
            CommandEntry {
                label_key: "cmd.remote_ssh",
                action_name: "workspace::RemoteSsh",
            },
            CommandEntry {
                label_key: "cmd.thread_history",
                action_name: "workspace::ThreadHistory",
            },
            CommandEntry {
                label_key: "cmd.continue_in_new_session",
                action_name: "workspace::ContinueInNewSession",
            },
            CommandEntry {
                label_key: "cmd.report_bug",
                action_name: "workspace::ReportBug",
            },
            CommandEntry {
                label_key: "cmd.about",
                action_name: "workspace::About",
            },
            CommandEntry {
                label_key: "cmd.check_updates",
                action_name: "workspace::CheckForUpdates",
            },
            CommandEntry {
                label_key: "cmd.open_settings",
                action_name: "workspace::OpenSettings",
            },
            CommandEntry {
                label_key: "cmd.open_settings_json",
                action_name: "workspace::OpenSettingsJson",
            },
            CommandEntry {
                label_key: "cmd.open_recent",
                action_name: "workspace::OpenRecent",
            },
            CommandEntry {
                label_key: "cmd.open_dialog",
                action_name: "workspace::OpenDialog",
            },
            CommandEntry {
                label_key: "cmd.shortcut_sheet",
                action_name: "workspace::ShortcutSheet",
            },
            CommandEntry {
                label_key: "cmd.usage_limits",
                action_name: "workspace::ShowUsageLimits",
            },
            CommandEntry {
                label_key: "cmd.usage_stats",
                action_name: "workspace::UsageStats",
            },
            CommandEntry {
                label_key: "cmd.copy_path_line",
                action_name: "workspace::CopyPathWithLine",
            },
            CommandEntry {
                label_key: "cmd.close_all_tabs",
                action_name: "workspace::CloseAllTabs",
            },
            CommandEntry {
                label_key: "cmd.toggle_pin_tab",
                action_name: "workspace::TogglePinTab",
            },
            CommandEntry {
                label_key: "cmd.open_in_vscode",
                action_name: "workspace::OpenInVsCode",
            },
            CommandEntry {
                label_key: "cmd.open_in_cursor",
                action_name: "workspace::OpenInCursor",
            },
            CommandEntry {
                label_key: "cmd.open_in_zed",
                action_name: "workspace::OpenInZed",
            },
            CommandEntry {
                label_key: "cmd.open_in_terminal",
                action_name: "workspace::OpenInTerminal",
            },
            CommandEntry {
                label_key: "cmd.show_ports",
                action_name: "workspace::ShowPorts",
            },
            CommandEntry {
                label_key: "cmd.show_inbox",
                action_name: "workspace::ShowInbox",
            },
            CommandEntry {
                label_key: "cmd.show_resources",
                action_name: "workspace::ShowResources",
            },
            CommandEntry {
                label_key: "cmd.show_cleanup",
                action_name: "workspace::ShowCleanup",
            },
        ]
    }

    pub(crate) fn get(&self, index: usize) -> Option<CommandEntry> {
        self.entries().get(index).copied()
    }
}

pub(crate) static COMMAND_REGISTRY: CommandRegistry = CommandRegistry;

#[cfg(test)]
mod command_registry_tests {
    use super::*;

    #[test]
    pub(crate) fn action_names_are_unique() {
        let entries = COMMAND_REGISTRY.entries();
        for (index, entry) in entries.iter().enumerate() {
            assert!(entries[..index]
                .iter()
                .all(|other| other.action_name != entry.action_name));
        }
    }

    /// パレットの行は全部、登録済みの action と訳のあるラベルを指す（名前の打ち間違いで
    /// 「押しても何も起きない行」を作らない）。
    #[gpui::test]
    fn every_palette_command_resolves(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            for entry in CommandRegistry.entries() {
                assert!(
                    cx.build_action(entry.action_name, None).is_ok(),
                    "登録されていない action: {}",
                    entry.action_name
                );
                assert_ne!(
                    i18n::translate(entry.label_key),
                    entry.label_key,
                    "訳の無いラベル: {}",
                    entry.label_key
                );
            }
        });
    }

    /// エージェント別の新規スレッド（B28）の行き先は、カタログのエージェントを 1 つずつ全部指す。
    #[test]
    fn agent_thread_actions_cover_the_catalog() {
        let mut labels = crate::workspace::editor_area::AGENT_THREAD_LABELS.to_vec();
        labels.sort_unstable();
        let mut catalog = acp_client::AGENT_LABELS.to_vec();
        catalog.sort_unstable();
        assert_eq!(labels, catalog);
    }
}
