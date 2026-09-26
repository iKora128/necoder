//! `necoder terminal …` — 起動中の necoder の端末（下ドックのタブと、Fleet の Task カードに置いた
//! 端末）を一覧し、画面を読み、文字を送り、出力が止まるまで待つ（G21）。
//!
//! 実体は GUI の制御 IPC（`control_ipc.rs` の `terminals` / `terminal_read` / `terminal_send` /
//! `terminal_wait`）。端末は GUI の中にしか無いので、全部 GUI が起動している時だけ動く。
//! **送る（send）は GUI の設定「CLI から端末へ入力を送る」が on の時だけ効く**（既定 off。
//! 送った文字はその端末でそのまま実行されるため）。一覧・読み取り・待機は何も起こさないので許可は要らない。
//! 形の出典: stablyai/orca@646e9a5 の `docs/site/content/docs/cli/reference.mdx` §Terminals（MIT。
//! list / read / send / wait の分け方）。ハンドルの作り方と待ち方は necoder の GUI に合わせて独立に書いた。

use crate::fleet::gui_request;
use anyhow::{Context as _, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// `necoder terminal` のサブコマンド 1 つ（名前・引数の書式・要旨）。
/// 引数が足りない時の「使い方」と `necoder skills get` の本文がこの表を共有する。
pub(crate) struct TerminalCommand {
    pub(crate) name: &'static str,
    pub(crate) arguments: &'static str,
    pub(crate) summary: &'static str,
}

/// `necoder terminal` の全サブコマンド（`run` の分岐と同じ並び）。
pub(crate) const TERMINAL_COMMANDS: &[TerminalCommand] = &[
    TerminalCommand {
        name: "list",
        arguments: "[task]",
        summary: "端末の一覧（ハンドル・どの Task か・下ドックか Task カードか・出力が止まってからのミリ秒）。task を渡すとその Task の端末だけ",
    },
    TerminalCommand {
        name: "read",
        arguments: "<terminal> [scrollback-lines]",
        summary: "画面に見えている行と、その上の scrollback（既定 0 行）を文字で出す",
    },
    TerminalCommand {
        name: "send",
        arguments: "<terminal> [--enter] <text...>",
        summary: "文字を送る。--enter で最後に Enter を押す。GUI の設定で許可した時だけ効く",
    },
    TerminalCommand {
        name: "wait",
        arguments: "<terminal> [idle-seconds] [timeout-seconds]",
        summary: "出力が idle-seconds（既定 2）止まるまで待つ。timeout-seconds（既定 600）を過ぎると失敗",
    },
];

/// `terminal_wait` の 1 要求で GUI に待ってもらう上限（GUI 側の上限 20 秒と同じ）。
const WAIT_REQUEST_LIMIT: Duration = Duration::from_secs(20);

fn usage_error(name: &str) -> anyhow::Error {
    match TERMINAL_COMMANDS
        .iter()
        .find(|command| command.name == name)
    {
        Some(command) => anyhow::anyhow!(
            "使い方: necoder terminal {} {}",
            command.name,
            command.arguments
        ),
        None => anyhow::anyhow!("{}", terminal_usage()),
    }
}

fn terminal_usage() -> String {
    let commands = TERMINAL_COMMANDS
        .iter()
        .map(|command| format!("{} {}", command.name, command.arguments))
        .collect::<Vec<_>>()
        .join(" | ");
    format!("使い方: necoder terminal <{commands}>")
}

/// `necoder terminal …` を処理したら true（GUI は開かない）。失敗は終了コード 1。
pub(crate) fn run_cli() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("terminal") {
        return false;
    }
    if let Err(error) = run(&args[1..]) {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
    true
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

/// 秒の引数（小数可）を読む。
fn parse_seconds(value: Option<&String>, default: f64, name: &str) -> Result<Duration> {
    let Some(value) = value else {
        return Ok(Duration::from_secs_f64(default));
    };
    let seconds: f64 = value
        .parse()
        .ok()
        .filter(|seconds: &f64| seconds.is_finite() && *seconds >= 0.0)
        .with_context(|| format!("{name} は 0 以上の秒数で（{value} は読めない）"))?;
    Ok(Duration::from_secs_f64(seconds))
}

/// `terminal` に続く引数を処理する（古い `ne` シムからは `necoder cli terminal …` 経由で来る）。
pub(crate) fn run(args: &[String]) -> Result<()> {
    let argument = |index: usize| args.get(index).map(String::as_str);
    match argument(0) {
        Some("list") => {
            let params = match argument(1) {
                Some(task) => json!({ "task_id": crate::fleet::resolve_task(task)?.id }),
                None => json!({}),
            };
            print_json(&gui_request("terminals", params)?);
        }
        Some("read") => {
            let Some(terminal) = argument(1) else {
                return Err(usage_error("read"));
            };
            let scrollback: u64 = match argument(2) {
                Some(value) => value
                    .parse()
                    .with_context(|| format!("scrollback-lines は行数で（{value} は読めない）"))?,
                None => 0,
            };
            let screen = gui_request(
                "terminal_read",
                json!({ "terminal": terminal, "scrollback": scrollback }),
            )?;
            print!("{}", screen_text(&screen));
        }
        Some("send") => {
            let Some(terminal) = argument(1) else {
                return Err(usage_error("send"));
            };
            let (enter, text) = parse_send(&args[2..]);
            if text.is_empty() && !enter {
                return Err(usage_error("send"));
            }
            print_json(&gui_request(
                "terminal_send",
                json!({ "terminal": terminal, "text": text, "enter": enter }),
            )?);
        }
        Some("wait") => {
            let Some(terminal) = argument(1) else {
                return Err(usage_error("wait"));
            };
            let idle = parse_seconds(args.get(2), 2.0, "idle-seconds")?;
            let timeout = parse_seconds(args.get(3), 600.0, "timeout-seconds")?;
            print_json(&wait_idle(terminal, idle, timeout)?);
        }
        _ => anyhow::bail!("{}", terminal_usage()),
    }
    Ok(())
}

/// 出力が `idle` 止まるまで待つ。GUI には 20 秒ずつ待ってもらい、足りなければ繰り返す
/// （1 要求の応答待ちに上限があるため）。シェルが終わった端末は待たずに返す。
fn wait_idle(terminal: &str, idle: Duration, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        let result = gui_request(
            "terminal_wait",
            json!({
                "terminal": terminal,
                "idle_ms": idle.as_millis() as u64,
                "timeout_ms": remaining.min(WAIT_REQUEST_LIMIT).as_millis() as u64,
            }),
        )?;
        let reached = result.get("idle").and_then(Value::as_bool) == Some(true);
        let exited = result.get("exited").and_then(Value::as_bool) == Some(true);
        if reached || exited {
            return Ok(result);
        }
        anyhow::ensure!(
            started.elapsed() < timeout,
            "terminal wait が timeout: {terminal} の出力が {:.1} 秒止まらなかった",
            idle.as_secs_f64()
        );
    }
}

/// `send` の残りの引数 → (Enter を押すか, 送る文字)。`--enter` はどこに置いてもよく、
/// `--` の後ろは全部文字として扱う（`--enter` という文字そのものを送りたい時）。
fn parse_send(args: &[String]) -> (bool, String) {
    let mut enter = false;
    let mut words = Vec::new();
    let mut literal = false;
    for argument in args {
        if !literal && argument == "--" {
            literal = true;
        } else if !literal && argument == "--enter" {
            enter = true;
        } else {
            words.push(argument.as_str());
        }
    }
    (enter, words.join(" "))
}

/// `terminal_read` の応答 → 端末の文字（scrollback → 画面の順）。画面の下の空行は落とす。
fn screen_text(screen: &Value) -> String {
    let lines = |key: &str| -> Vec<String> {
        screen
            .get(key)
            .and_then(Value::as_array)
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut all = lines("scrollback");
    let mut visible = lines("lines");
    while visible.last().is_some_and(String::is_empty) {
        visible.pop();
    }
    all.extend(visible);
    let mut text = all.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn send_takes_enter_anywhere_and_literal_text_after_double_dash() {
        assert_eq!(
            parse_send(&strings(&["--enter", "npm", "test"])),
            (true, "npm test".to_string())
        );
        assert_eq!(
            parse_send(&strings(&["cargo test", "--enter"])),
            (true, "cargo test".to_string())
        );
        assert_eq!(
            parse_send(&strings(&["--", "--enter"])),
            (false, "--enter".to_string())
        );
        assert_eq!(parse_send(&strings(&["--enter"])), (true, String::new()));
    }

    #[test]
    fn screen_text_puts_scrollback_first_and_drops_blank_tail() {
        let screen = json!({
            "scrollback": ["old 1", "old 2"],
            "lines": ["$ cargo test", "ok", "", ""],
        });
        assert_eq!(screen_text(&screen), "old 1\nold 2\n$ cargo test\nok\n");
        assert_eq!(screen_text(&json!({ "lines": ["", ""] })), "");
    }

    #[test]
    fn seconds_accept_fractions_and_reject_garbage() {
        let half = "0.5".to_string();
        assert_eq!(
            parse_seconds(Some(&half), 2.0, "idle-seconds").expect("読める"),
            Duration::from_millis(500)
        );
        assert_eq!(
            parse_seconds(None, 2.0, "idle-seconds").expect("既定"),
            Duration::from_secs(2)
        );
        assert!(parse_seconds(Some(&"-1".to_string()), 2.0, "idle-seconds").is_err());
        assert!(parse_seconds(Some(&"soon".to_string()), 2.0, "idle-seconds").is_err());
    }

    #[test]
    fn usage_comes_from_the_command_table() {
        assert_eq!(
            usage_error("read").to_string(),
            "使い方: necoder terminal read <terminal> [scrollback-lines]"
        );
        let usage = terminal_usage();
        for command in TERMINAL_COMMANDS {
            assert!(usage.contains(command.name));
        }
    }
}
